# `tools@1.4.0`

The tool surface of one assistant as **one node with one contract**: `tool_call` in,
`tool_result` out.

That sentence is the whole template. Everything below is either a consequence of it or an
honest note about what it does not yet cover.

Since 1.2.0 the node answers a second question, and it is a question about **itself**: a
cell that uses tools names them, and the hive hands back their schemas. That is the other
half of the same sentence -- a surface that decides which cell serves a call, and never
tells anyone which calls there are to make, is a contract with a hole in it that every
caller patches by hand.

## The contract

| direction | lane | what it carries |
|---|---|---|
| in | `tool_call` | one call a brain decided to make. `hop.tool_name` says which tool; the body is the `tool_call` turn a dispatcher split out of the brain's bundle. |
| out | `tool_result` | what the tool answered -- **whatever the tool was**. |
| out | `build` | a structural wish, or a manifest being submitted -- the one lane on which a tool of this surface reaches OUT of the assistant. |
| in | `in_build_result` | the answer to that: a draft manifest, or the receipt of a submission. |
| in | `in_schemas` | the names a caller declares it uses -- `{"tools": ["web_search", "web_fetch"]}`, or `["*"]` for everything. |
| out | `tool_schemas` | their declarations, plus the names the hive does not have. |

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

`params.required_drains` pairs all three directions in the lane form: *a caller that sends
me `tool_call` must subscribe to `tool_result`*, *a level that sends me `in_build_result`
must subscribe to `build`*, and *a caller that sends me `in_schemas` must subscribe to
`tool_schemas`*. A mutation that wires one half without the other is refused
with `required_drain_missing` before anything is staged. There is no topology in which
handing this hive a call and not taking the answer back is the intended shape -- the tool
would run, cost whatever it costs, and answer into nothing while the brain waits for a turn
that was produced and delivered nowhere.

## Why it is sealed

`params.ports` is `[]`. The hive path is the only address, and that is the point rather
than a restriction: **the tool surface is a contract, not a set of addresses.** If a caller
could draw an edge to a single tool cell, the set of tools would live in the caller's edge
table, and every change to it would be a change to the caller. Sealed, adding a tool
is one occupant directory, two internal edges -- a door and an exit -- and one row in the
`schemas` cell's table; the caller's single edge does not move.

This is also what makes the acceptance in GH #286 reachable: replacing three tool cells
with one code-executing cell is a `swap_nodes` on this node. The caller does not change,
because the contract does not change.

## The occupants

| directory | cell type | reached on | `params.sandbox` |
|---|---|---|---|
| `bash/` | `bash` | `hop.tool_name == 'bash'` | `{"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}` |
| `web_fetch/` | `web_fetch` | `hop.tool_name == 'web_fetch'` | none |
| `web_search/` | `web_search` | `hop.tool_name == 'web_search'` | none |
| `file/` | `file` | `hop.tool_name == 'file'` | none |
| `edit/` | `edit` | `hop.tool_name == 'edit'` | none |
| `build-draft/` | `code` | `hop.tool_name == 'build_topology'` | `{"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}` |
| `build-apply/` | `code` | `hop.tool_name == 'apply_manifest'` | `{"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}` |
| `schemas/` | `code` | the `in_schemas` lane | `{"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}` |
| `unknown/` | `code` | nothing else fired | `{"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}` |

**Every occupant in this table is reached**, and since 1.3.0 there is no row left saying
*nothing* in the third column.

`file` and `edit` share ONE root (`TOOLS_FILE_ROOT`, § *The environment surface*), and that
is a decision rather than a convenience: one assistant has one file root, and a tool that
could edit what its neighbour cannot read would be a boundary nobody could state.

`build-draft` and `build-apply` are the two halves of one round -- **and since 1.4.0 they
are named as one** (§ *What `1.4.0` changed*). They carry the tightest block in the
tree: they compute nothing, reach nothing and store nothing. What they do is carry -- a
wish out and a draft back, then that same draft out again and a receipt back. Both phases
of both cells are recognised POSITIVELY, and the correlation of the round travels in the
CONTEXT (`build_call_id`), because `hop` is a one-hop compartment and the builder is eight
hops away.

Every occupant is a copy of a shape the library already carries -- `bash` after the coder
pipeline's runner, `web_fetch` after the daily digest's fetcher, `web_search`, `file` and
`edit` after the cell types of the same names -- so no cell type is invented here. The
converse does not hold, and 1.3.0 is where that is written down: a cell type existing is
not a reason for a directory here. `unknown` is not a tool at all; it is the subject of the dispatch section
below, and `schemas` is not a tool either: it answers a lane of its own and no `tool_call`
ever reaches it.

**The `none` rows are the shipped truth, not an oversight**, and they are two different
sentences. For `web_fetch` and `web_search`, egress is what those cells are for:
`network: "deny"` would turn them off, and a process sandbox has no notion of an allowed
host. For `file` and `edit`, a process sandbox has nothing to bound at all -- those cells do
their own I/O rather than starting a runner, so the fence is somewhere else:
`params.base_path`, checked lexically and again after canonicalisation. Either way the rows
are written down as they are so that the blast radius of this hive is a thing one can read
instead of a thing one discovers. `bash` carries the block GH #286 measured on the shipped instance,
verbatim.

### RETRACTED in 1.3.0: ~~`mcp/` and `vault/` stand here and are not wired~~ ([#547](https://github.com/mmeyerlein/meclaw/issues/547))

Until 1.3.0 two further directories stood in this table with **nothing** in the *reached
on* column, and this section explained at length how to wake them. Both are gone, and the
sentence that carried them is retracted rather than quietly deleted
(`docs/development-rules.md` § 3).

> **A cell type is not a tool.** The tools hive holds the cells this agent's tool calls
> reach. Every shipped cell type does not belong here by virtue of being a cell type: a
> `vault` answers its broker and stands in the broker's hive, and an `mcp` bridges one
> named server and is wired when somebody names it. Neither is wired here any more,
> because an occupant nobody can reach teaches nothing.

The argument the old text made was true about the SUBSTRATE and wrong about the LIBRARY. It
is true that activity here is derived from the edges alone -- a cell no edge touches is an
island, the boot registers it, keeps its `cell_id` and never spawns it -- and shipping a
cell asleep is a real technique that costs one `add_edges` pair to undo. What does not
follow is that a shipped TEMPLATE should do it. A template in this library is a worked
example; an occupant nobody can reach is a placeholder with documentation attached, and the
documentation had grown longer than the thing it documented.

The `vault` case is the sharper of the two, because it taught the opposite of the shipped
arrangement. A vault answers exactly ONE sender -- its broker -- and attests its own inbound
edges before it will accept key material. The arrangement that works is the one
[`templates/access`](../access/) already ships, where the vault stands in the capability
broker's own hive beside the `invoke` cell that is allowed to spend it. A `vault/` directory
in a tools hive says a vault is a tool, and the paragraph under it then had to spend itself
un-saying that.

The `mcp` case is the same shape for a different reason: an `mcp` cell bridges ONE MCP
server, nobody had named the server, and a long-running cell wired at a loopback
placeholder reconnects to nothing forever. Naming the server is a decision, and a template
that cannot make it should not ship the half of the answer that looks like it did.

**Neither capability is lost, and neither is hard to draw.** An assistant that talks to a
named MCP server adds an `mcp` cell to this hive the way any tool is added -- one occupant
directory, two edges, one row in the `schemas` cell's table, all three in one diff (§ *The
declarations*). An assistant that needs a vault gets one where a vault belongs, in the
broker's hive. `workshop/corpus/13-mcp-lane/` is the worked example for the first, and it
is a whole colony rather than a directory nobody reaches.

## What `1.4.0` changed: the build pair is named as a pair

[#554](https://github.com/mmeyerlein/meclaw/issues/554). The two occupants that carry a
build were called `build` and `apply`, and the tree said nothing about the fact that they
are two halves of ONE round: one fetches a draft, the other submits it, and a reader of
the graph had to know the story before the two names looked related at all.

They are `build-draft/` and `build-apply/` now. **`draft` names what the first half
delivers** -- the sentence *"This is a DRAFT"* is verbatim in its `tool_result` -- and the
shared `build-` prefix is what makes the pair visible where a reader actually meets it: in
the nine edges of `params.graph` and in a colony's registry listing, both of which sort the
two neighbours together instead of scattering them alphabetically between `bash` and
`edit`.

**The tool names the model sees did not move**, and that is the whole boundary of this
change: `build_topology` and `apply_manifest` are the contract surface a brain's
`system.tools` names and a door dispatches on (`hop.tool_name`), and a cell name is not a
tool name. The `schemas` occupant's table, the two doors' conditions and every caller's
prompt are byte-identical across this version.

What moved is this hive's own address space, which is why it is the second digit: a
mutation or an `override_params` path that named `<assistant>/tools/build` names
`<assistant>/tools/build-draft` from 1.4.0 on. A generation already grown from 1.3.0 keeps
the names it was grown under -- the template library is not on the running path of a booted
colony, and the pair lands with the next instance built.

## The declared blast radius

The table above is four separate files, and what they add up to is a fifth fact nobody was
writing down. `template.json` writes it down, in two machine-readable blocks.

**`sandbox_union` -- the widest value of every axis, over every occupant.**

| axis | union | who sets it |
|---|---|---|
| `trust` | `full` | the four occupants that declare no block: `web_fetch`, `web_search`, `file`, `edit` |
| `network` | `allow` | the same four |
| `filesystem` | `unrestricted` | the same four |

A union is not an average. `bash`, `unknown`, `build-draft`, `build-apply` and `schemas` are as
restricted as the library gets, and they tighten **nothing** about the hive as a whole: what
they bound is what happens once dispatch has already chosen them. Any single unsandboxed
occupant alone puts every axis at its widest, and that is the honest reading of what handing
this hive a `tool_call` can set in motion.

Until 1.3.0 the sum also counted `mcp` and `vault`, on the argument that a union leaving a
directory out is the invisibility this declaration exists to end. That argument was right,
and it is why the two were counted for as long as they stood here. They no longer stand
here ([#547](https://github.com/mmeyerlein/meclaw/issues/547)), so the union is over the
occupants that actually answer -- narrower on no axis, because the four that set it are
unchanged, and honest about what it is a union over.

That is uncomfortable to read, and it is supposed to be. The radius is not new -- GH #286
measured it on the shipped instance -- it was merely spread across files where nobody had to
look at it whole. **The union is what a replacement is measured against: a code-executing
cell that needs exactly this is not a widening; one that needs more is.**

**`reentrancy` -- one entry per occupant, and since 1.2.0 they are not all the same word.**

| occupant | reentrant | why |
|---|---|---|
| `bash` | yes | one-shot, no session state, no working directory carried between calls |
| `web_fetch` | yes | stateless request/response, one GET per call |
| `web_search` | yes | stateless request/response, no cursor, no pagination state |
| `file` | yes | one op per call, no handle and no cursor between calls |
| `edit` | **no** | a read-modify-write with no lock; two edits of one file race in the filesystem, and the caller cannot see that |
| `unknown` | yes | it formats a string; there is no state to reach |
| `schemas` | yes | it reads names and writes declarations out of a table compiled into its own script |
| `build-draft` | yes | it reads a hop and a turn and writes a turn; the correlation runs over the round's own context |
| `build-apply` | yes | the same, and the ORDER of two submissions is the colony's to decide at the door, never this cell's to assume |

`params.max_concurrency` is a queue and not a serialisation wherever the verdict is `yes`:
the call over the cap waits and still answers on its own `tool_result`. **`edit` is the one
exception, and its cap of one is the verdict, not a tuning.** An edit is a read-modify-write
without a lock and without tempfile+rename (`docs/cell-types.md` § `edit`), so two edits of
the same path race at the filesystem -- a hazard a caller cannot see from outside. This
occupant serialises rather than asking the caller to. A brain may still fan a round out
across the other occupants; the edits in that round run one after another.

Every entry is stated even where the answer repeats, because the hazard this declaration
exists for is a **swap** that quietly makes a parallel tool round sequential -- and an
occupant nobody declared is exactly the one whose serialisation surprises the caller. The
check runs in both directions: every occupant directory has an entry, and every entry names
a directory that exists, so an entry cannot outlive the cell it described.

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

Every edge that does it is `from` or `to` the hive path itself -- no caller can name any of
them, because `params.ports` is `[]`.

**The doors, out of `.`.** Ordinary conditioned edges, one per tool, each asking for the
accepted lane first and then narrowing *within* it on `hop.tool_name`:

```json
{ "from": ".", "to": "./bash",
  "condition": "has(hop.route) && hop.route == 'tool_call' && has(hop.tool_name) && hop.tool_name == 'bash'" }
```

They are mutually exclusive by construction -- equality tests against distinct literals --
so a known tool name selects exactly one of them. Overlapping positives would not be an
error, they would simply stay fan-out; none is authored, and none should be.

**The lane term is not decoration.** An occupant's answer travels back out through this
same hive path, so a door that asked only about `hop.tool_name` would also be offered every
answer -- and an answer that happened to carry the name would be handed straight back to
its own sender, round after round until the TTL ran out. Asking for the lane first says
what the door is actually for: *inbound calls*, not everything that passes the hive path.

Then the guarded default, which is the door worth reading twice:

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

**The exits, into `.`.** One per occupant, each stamping the outward lane that occupant's
answer belongs on -- `tool_result` for every tool, `tool_schemas` for `schemas`:

```json
{ "from": "./bash", "to": ".",
  "condition": "has(hop.operation) && hop.operation == 'bash'",
  "modifier": { "set_hop": { "route": "'tool_result'" } } }
```

Ordinary conditioned fan-in edges, and **none of them is a default**. Default suppression
is per *sender*; these senders are the occupants, not the hive path, so they cannot
interact with the call-side default at all. The condition reads `hop.operation` because
that is what each tool cell stamps on every emission it has -- success, non-zero exit, 404,
timeout alike -- and `unknown` stamps `operation: "unknown"` for the same reason: the exit
says *which occupant answered*, and the lane says *what kind of message it is*.

**`file` and `edit` name more than one value, and one of the values is `unknown`.** Those
two cells label the operation they RAN, so their exits list the four and the two:

```json
{ "from": "./edit", "to": ".",
  "condition": "has(hop.operation) && (hop.operation == 'find_replace' || hop.operation == 'insert_at_line' || hop.operation == 'unknown')",
  "modifier": { "set_hop": { "route": "'tool_result'" } } }
```

`unknown` is in both lists because that is the label those cells put on a call whose
arguments did not parse far enough to say which op was meant -- a refusal that has to reach
the caller like every other. It cannot be confused with the `unknown/` occupant: suppression
and matching are per SENDER, and these edges leave `./file` and `./edit`.

**And one thing an author adding a tool should keep anyway.** The lane guard above makes it
safe, but an answer still has no business carrying the key the dispatch runs on: a cell
emission mints a fresh hop, and no shipped tool cell writes `tool_name` into it
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

## The declarations

**A tool schema now lives in this hive, and until 1.2.0 this file said the opposite.** The
retracted sentence was *"Tool schemas belong in the calling brain's `system.tools`... a
schema here would be a second copy of the same list, and the two would drift on the first
tool anyone added."* The first half of that was right about the DESTINATION and wrong about
the SOURCE, and the second half described the state it produced rather than prevented: with
no source in the hive, every caller typed the list into its own prompt, and adding a tool by
mutation meant editing every one of those prompts by hand. There were as many copies as
there were callers, and no two of them had to agree. There is one copy now, it is here, and
what a caller keeps is the NAMES it uses -- which is a decision, not a copy.

The occupant is `schemas/`, a `code` cell on a lane of its own.

| | |
|---|---|
| reached on | `in_schemas` at the hive path -- never a `tool_call`, never a `tool_name` |
| asked with | `{"tools": ["web_search", "web_fetch"]}`, or `{"tools": ["*"]}` for everything |
| answers on | `tool_schemas`, with `schemas[]` and `unknown[]` |

```json
{ "schemas": [ { "name": "web_search",
                 "description": "Query the configured search endpoint ...",
                 "parameters": { "type": "object",
                                 "properties": { "query": { "type": "string" } },
                                 "required": ["query"] } } ],
  "unknown": ["telepathy"] }
```

**A name the hive does not have comes back in `unknown`, never dropped**, and
`hop.error_code` is `tool_unknown` when that list is non-empty. A menu that is silently one
tool short is a model that never calls it and an author who never learns why; a partial
answer is still an answer, so the schemas that WERE found travel beside the names that were
not. An `in_schemas` request carrying no `tools` slot at all is a third state and gets its
own code, `tools_missing` -- an absent list and an empty one are two different requests, and
a caller that declares no tools is entitled to an empty menu rather than to everything.

**Where the schemas come from, and why they are written down here rather than derived.** No
tool cell in this library publishes an argument schema anywhere a machine can read. Each one
declares a `contract` -- which body slots it consumes, which hop keys it emits -- and that is
a different document: `contract` answers *what does the substrate carry*, a schema answers
*what may I fill in*. The only description of the arguments that exists today is prose, in
each occupant's `description.consumes_meaning` and its example line. So this table is the
**first** machine-readable copy rather than a second one, and it is deliberately compiled
into the `schemas` cell's own script rather than kept in a store: a store would be a second
cell to reach, a second round trip inside the hive, and a `cell.db` whose content nobody
reviews in a diff. In the script, adding a tool stays what it already was -- one occupant
directory, two edges and one row, all three in this one directory, all three in one diff.

The two halves cannot drift apart in silence: a test walks the dispatch graph and requires
that every tool a door names has a row, and that every row names a tool a door dispatches
to. Until 1.3.0 the two unwired occupants were in neither list, by the same rule -- a schema
for a tool no edge reaches would offer a model a call that goes nowhere. With them gone,
the rule has nothing left to except, which is the better shape for a rule to be in.

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

### Asking for the declarations

The second pair, for a caller that wants its menu from here instead of from a typed list.
It is a lane pair like the first one, and `params.required_drains` refuses a mutation that
draws only half of it:

```json
[
  { "from": "./collector", "to": "./tools",
    "condition": "has(hop.route) && hop.route == 'schemas'",
    "modifier": { "set_hop": { "route": "'in_schemas'" } } },
  { "from": "./tools", "to": "./collector",
    "condition": "has(hop.route) && hop.route == 'tool_schemas'" }
]
```

**What a caller sends.** A body with one slot:

```json
{ "tools": ["web_search", "web_fetch"] }
```

`["*"]` asks for every tool the hive has. The list is the caller's own declaration -- the
names its template said it uses -- and the hive keeps no record of who asked: whoever
designed that template decided what it uses, and that decision belongs where the rest of its
contract is, not in a subscriber table over here that drifts on the first rewiring.

**What comes back**, on `tool_schemas`, in the body:

| slot | what it is |
|---|---|
| `schemas[]` | one `{name, description, parameters}` per name the hive has, in the order asked (sorted, for `*`) |
| `unknown[]` | the names it does not have, verbatim |
| `messages[]` | empty -- this is not a turn and no model reads it |

and in the hop: `operation: "schemas"`, `schema_count`, `unknown_count`, and `error_code`
`tool_unknown` (at least one name was not found) or `tools_missing` (the request carried no
`tools` slot at all).

**The answer is provider-neutral on purpose.** `{name, description, parameters}` is not the
shape any model provider wants; wrapping it -- `{"type": "function", "function": {...}}` for
an OpenAI-dialect provider -- is the caller's job, because the caller is the one that knows
its provider. A hive that wrapped would be a hive that had to be told which provider its
caller talks to, which is a second thing a caller has to tell it and a first thing this hive
would be wrong about.

**When to ask.** Not per turn: the answer is a durable `system.*` slot on the caller's side,
written once and read by every turn after it. Asking it AGAIN is how a caller learns that a
tool was added, and nothing here pushes -- the hive answers questions and raises nothing of
its own.

That makes the ask a **tick** rather than a birth, and the shipped caller says so out loud:
the substrate hands a cell no message at spawn, so there is no moment called "start-up" at
which anything could ask. `collector` carries a `timer` for it (`MENU_CRON`), the
first firing is the first ask, and every later one costs two selects over unchanged data on
this side -- see [`templates/collector/README.md`](../collector/README.md) § *The menu is
asked for*.

## The environment surface

`template.json`'s `requires.env` declares the two tokens the `web_search` occupant binds,
and says which value each one fills:

| key | binds | required |
|---|---|---|
| `SEARCH_ENDPOINT` | `web_search`'s `params.endpoint`, written `${SEARCH_ENDPOINT:-http://127.0.0.1:8080/search}` | no |
| `SEARCH_API_KEY` | `web_search`'s `params.api_key`, written `${SEARCH_API_KEY:-}` | no |
| `TOOLS_FILE_ROOT` | `params.base_path` of BOTH `file` and `edit`, written `${TOOLS_FILE_ROOT:-/tmp}` | no |

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

**`TOOLS_FILE_ROOT` is the one whose default is a real reach, and that is the honest part.**
`base_path` must name a directory that EXISTS or the cell refuses to spawn, and the only
directory that exists on every machine is the scratch one. So unset, this assistant's file
surface is rooted at `/tmp`: nobody's mistake, nothing anywhere reports it, and the same
argument as the search endpoint one line up -- which is why it is written down rather than
discovered. Set it to the directory this assistant may actually touch.

`MCP_ENDPOINT` and `TOOLS_VAULT_BROKER` were declared here until 1.3.0 and are gone with
the two occupants they configured ([#547](https://github.com/mmeyerlein/meclaw/issues/547)).
They were never defaults anybody fell back on; they were the questions that kept two cells
unwired, and a template's environment surface is not the place to ask a question the
template cannot use the answer to.

No token carries a value in this tree and none ever should: what is declared is the name of
a secret or the address of a service, never the secret itself.

## What does NOT live here

- ~~**Tool schemas.**~~ **RETRACTED in 1.2.0** ([#464](https://github.com/mmeyerlein/meclaw/issues/464)).
  This list used to say a schema here would be a second copy of the calling brain's
  `system.tools`. The destination was right and the source was wrong: with no schema in the
  hive there was no first copy, only one per caller, typed by hand and free to disagree. The
  schemas live in `schemas/` now and a caller asks for the ones it declared -- § *The
  declarations*. What still does not live here is the DECISION: which tools a caller uses is
  its own template's business, and this hive keeps no list of who asked.
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
