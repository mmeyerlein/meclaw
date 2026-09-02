# `talky@4.6.1`

A whole conversational agent as one template. Three referenced units under one hive:
[`session-keeper`](../session-keeper/), [`collector`](../collector/) and
[`dispatcher`](../dispatcher/) -- each carrying its template's own name -- plus an
`llm` brain, the sidecar splitter and one error collector. No new cell type, no Rust.

**The Egon rollout wired this by hand.** Keeper in the ingress, collector at the seam,
dispatcher for the fan-out, the close batch out to the write port -- thirty-one edges,
each of them a decision that had already been made in a README. That is the definition of a
composite: a recurring unit that should be instantiated, not re-derived. Here it is one
`add_nodes` plus the four port edges the parent has to draw anyway.

## What it delivers

- **One id per conversation, minted once.** The keeper stamps every inbound turn with
  the generation its channel is currently in; the internal edge promotes
  `hop.session_id` to context, and that promotion *is* the stamp. Everything downstream
  reads it and nobody else mints one.
- **The seam, already bounded.** The collector hands the assembled context to the brain
  over ONE edge, and that edge carries the two things the loop needs: the iteration
  counter and `restore_ttl`. A tool round is a dozen routing hops; without the restoring
  edge the fifth round dies mid fan-in with nothing emitted towards the surface.
- **A tool round that only needs its tools.** `brain -> splitter -> dispatcher -> (your
  tools) -> collector -> brain` is pre-wired except for the one lane that is genuinely
  per-instance: which cell answers to `web_search`. Adding a tool is one edge pair, never
  a topology change.
- **A close that hands the day on, once.** When a generation ends, the collector's batch
  leaves on the write port and that is the whole of it: the parent decides where a day
  belongs. Since `4.3.0` (GH #447) nothing summarises the batch inside this hive -- the
  member above the talky turns the close batch into the memory hive's close pass, and
  what the next generation needs of the last one comes back with the recall bundle of a
  turn, not as a `system.handover` slot the composite writes behind its own back.
- **One place errors leave from.** The brain's failed inference, its content filter and
  the session-keeper's store refusal -- three failure lanes from two cells -- fan into
  `./errors` and leave as one normalised report. A parent drains one edge, not three
  lanes.

## Cells

| path | type | from |
|---|---|---|
| `session-keeper/{stamp,close,sessions,night}` | `code`, `code`, `store`, `timer` | `session-keeper` **(sealed)** |
| `collector/{assemble,window,menu-clock}` | `code`, `store`, `timer` | `collector` **(sealed)** |
| `dispatcher` | `code` | `dispatcher` (a single-cell template) |
| `brain` | `llm` | this template |
| `splitter` | `code` | this template |
| `errors` | `code` | this template |

**The braces are an inventory, not an address list.** The two sealed sub-units declare
`params.ports: []`, so `./session-keeper` and `./collector` are the only sub-unit
addresses an edge from outside may name; `./session-keeper/stamp` and
`./collector/assemble` are refused with `hive_port_boundary`. Which cell inside picks the
message up is decided by the `in_` lane the edge sets, by the hive's own door edges. That
is what lets the inside of a sub-unit change without touching a caller.

### How the sub-units are referenced: by name and version (GH #277)

The three sub-units are **references**, not copies. Each of the three directories holds
one `config.json` and nothing else:

```json
{"cell": {"type": "ref", "template": "collector@3.5.0"}}
```

At instantiation the referenced template's tree takes that position, so the instance is
byte-for-byte the tree the copies used to produce -- and every cell inside it now records
the template it really came from: `collector/assemble` is stamped with the `collector` version it was grown from, with
`talky@4.6.1` above it in its provenance chain.

**The library has to carry the three.** A reference resolves against the colony's template
registry, so `collector`, `session-keeper` and `dispatcher` have to sit in
the same `templates/` directory as `talky` -- as they do in the shipped library. A tree
that copied `talky` alone gets `template not found` at the mutation, not at boot.

**The version is pinned on purpose.** A bare `collector` would resolve to whatever the
highest version on disk happens to be, so a standalone bump would silently re-point this
composite. The pin makes the composite say which version it was built against; moving it
is a `talky` bump, in the same commit.

Until GH #277 the sub-units lived here as byte copies of their `config.json` files, held
against their sources by a byte-identity pin. Its successor is
`crates/meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs`: the two
golden manifests prove the instantiated bytes did not move, and
`a_cell_inside_talky_is_stamped_with_its_own_template_and_names_talky_above_it` proves
the origin is recorded.

## Lanes

`params.ports` is empty (GH #228). **The address is the composite's own path**; what
a caller wants rides on `hop.route`, and the door edges inside decide which cell that
means. The four essential lanes are wired in the SAME mutation that instantiates the
composite -- an island without a crossing edge derives inactive and its timer never
spawns.

| lane | direction | what travels |
|---|---|---|
| `in_turn` | in | the surface turn. The edge MUST promote the channel identity to `context.channel`, and the round to `context.audience_set` if closed sessions are to reach a memory |
| `answer` | out | the finished turn. **Three** sorts since `collector@2.1.1`: a real answer, a round that hit `max_iter` (`hop.round_capped`, and since `collector@3.5.0` `hop.partial == "1"` with a named partial answer as its last turn, #570), and a turn the store refused to let be assembled (`hop.degraded`, which carries no `round_capped`) |
| `write` | out | the closed session as one batch |
| `error` | out | a normalised failure report. **MUST** be wired |

The rest, each optional and each still at the same address:

| lane | direction | what travels |
|---|---|---|
| `tool` | out | a tool call for a cell you wired; `hop.tool_name` says which |
| `turn_write` | out | **one message per turn, never a batch** (GH #298): one `user`/`assistant` turn, `hop.turn_id` = `<session_id>#<index>`, `hop.turn_index` and `hop.happened_at`. On unless the instance switches it off -- see "Per-turn episodes" |
| `recall` | out | a memory read this turn needs |
| `in_tool` | in | one tool result coming back |
| `in_advice` | in | an advisor's answer coming back |
| `in_bundle` | in | a memory bundle coming back |
| `in_memory_call` | in | a memory tool call handed back into the composite |
| `in_thread_call` | in | a `thread_recall` tool call handed back into the composite (since 3.0.1) |
| `in_sweep` | in | an operator-forced session sweep |
| `in_prune` | in | the age cut of the context window. **Paired**: see `prune` |
| `prune` | out | the report `in_prune` answers with -- one message per cut session (`hop.pruned_turns`, `hop.pruned_rounds`, `hop.prune_boundary`, `hop.session_id`), or a single zero report ("pruned nothing") when nothing was behind the gate. Since 3.0.6 |
| `in_round_sweep` | in | the other operator lane of the context window: a round that ran out of iterations |
| `in_pack` | in | a durable `system.*` slot for the brain: `identity`, `persona`, `handover` or `instructions`, and nothing else. **Paired**: see `pack_ack`. Since 4.4.0 |
| `pack_ack` | out | the receipt `in_pack` answers with, accepted and refused alike: `hop.pack_owner`, `hop.pack_slots`, `hop.error_code` (empty, `slot_unknown` or `pack_empty`), `hop.pack_unknown`. Since 4.4.0 |
| `schemas` | out | the tool names this agent declares it uses (`{"tools": [...]}`), for a tools hive's `in_schemas` door. It leaves on a TICK, not per turn. **Paired**: see `in_menu`. Since 4.5.0 |
| `in_menu` | in | their declarations coming back, plus the names that hive had nothing under. Since 4.5.0 |
| `in_export` | in | a demand for the session ledger as a versioned document. It crosses to `./session-keeper` unchanged and the keeper's own walk answers it. **Paired**: see `dump`. Since 4.5.0 |
| `in_import` | in | one part of such a document, for a keeper that is already running. Same crossing, same hive, same pairing. Since 4.5.0 |
| `dump` | out | what the two transfer lanes produce: an export part (`hop.dump_kind == 'export_part'`) or the receipt of an applied one (`'import_receipt'`). **Drain it with a PLAIN `hop.route == 'dump'` test** -- an edge that also tested `dump_kind` reads as no drain under the `required_drains` probe. Since 4.5.0 |

### The sessions leave, and come back (`in_export` / `in_import`, GH #475)

The keeper inside this composite has carried a transfer lane since `session-keeper@2.1.0`,
and for one release nothing above it forwarded one. So the sessions -- the one table that
remembers which conversation belongs to which channel -- stayed behind on every rebuild,
and a member reborn from its own export greeted a person it had been talking to for a year
as a stranger. Three edges close that, and all three are pure transit:

```json
[
  { "from": ".", "to": "./session-keeper",
    "condition": "has(hop.route) && hop.route == 'in_export'" },
  { "from": ".", "to": "./session-keeper",
    "condition": "has(hop.route) && hop.route == 'in_import'" },
  { "from": "./session-keeper", "to": ".",
    "condition": "has(hop.route) && hop.route == 'dump'" }
]
```

**No modifier, deliberately.** The lane is named the same on both sides of every boundary
it crosses, so a `set_hop` here would rename a lane onto itself -- and worse, it would hide
the pairing from the drain probe that `required_drains` runs through the real edge
evaluator.

**This composite reads no part and judges none.** What may leave is the keeper's own walk;
the document format is the keeper's own (`meclaw-session-export/1`); the idempotency of a
repeated part is the keeper's own probe. What is added here is a door and an exit, which is
the whole of the finding.

**A refusal does not travel on `dump`.** The porter refuses on the keeper's `reject` lane,
which `./errors` already drains and normalises -- so an export that aborted mid-walk and a
brain that returned a 500 leave this composite on one lane, in one shape, and the parent
still drains one edge. Above this level, [`assistant`](../assistant/README.md) passes the
same three edges through and [`member`](../member/README.md) files the parts in its own
export sink; a member's whole recipe is in
[`../member/README.md`](../member/README.md) § *The export, and the one cell this level
owns*.

**`in_prune` and `prune` are one decision, and the substrate insists on it.** The
prune answers unconditionally -- the zero report is the case an operator most needs,
because "nothing was eligible" is an answer. `params.required_drains` pairs the two, so
a mutation that wires the ingress without a subscription to the report is refused with
`required_drain_missing` instead of dead-lettering one report per request:

```json
{"from": "<timer or operator>", "to": "./talky",
 "condition": "has(hop.route) && hop.route == 'cut'",
 "modifier": {"set_hop": {"route": "'in_prune'"}}},
{"from": "./talky", "to": "<log or drain>",
 "condition": "has(hop.route) && hop.route == 'prune'"}
```

Drain it with a **plain** `hop.route == 'prune'` edge. The pairing's probe carries
`hop.route` and nothing else, and an edge that also tests `hop.pruned_turns` is judged on
that empty hop -- with two outcomes, not one. A test that **evaluates** (`has(hop.pruned_turns)`)
comes back false, the edge reads as no drain, and the mutation is refused. A test that
**errors** on the absent key is treated as UNKNOWN and counted as a drain
(`unwrap_or(true)`, `crates/meclaw-colony/src/mutation/required_drains.rs`): deliberate
conservatism, so the gate never invents a missing drain -- but it also means such an edge
passes the gate and can still skip the report at runtime. Keep the edge plain and neither
case arises.

**The lane names are the contract, the cell names are not.** `session-keeper`,
`collector`, `dispatcher` and `errors` are implementation: they may be renamed, split
or replaced in a version bump and no parent notices, because no parent addresses them.
A lane may not -- removing or renaming one is a breaking change to every caller, and it
gets a CHANGELOG Breaking entry and a new major version, not a patch. Which lanes exist
and what each is for is in `params.contract`, in the template itself.


Plus, per instance, the two **advisor lanes** to an agent core -- see below.

```json
{"from": "<surface>", "to": "./talky",
 "condition": "has(hop.user_id) && int(hop.user_id) == 12345",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"channel": "hop.chat_id",
                              "audience_set": "'[\"member:alex\",\"agent:scribe\"]'"}}},
{"from": "./talky", "to": "<reply sink>",
 "condition": "has(hop.route) && hop.route == 'answer' && !has(hop.round_capped) && !has(hop.degraded)"},
{"from": "./talky", "to": "<day archive or memory>",
 "condition": "has(hop.route) && hop.route == 'write'",
 "modifier": {"set_hop": {"route": "'in_batch'"}}},
{"from": "./talky", "to": "<drain or alarm>",
 "condition": "has(hop.route) && hop.route == 'error'"}
```

**The channel promotion is the parent's duty, and it is not optional.** Without
`set_context: {"channel": ...}` on the ingress edge every chat of the colony lands on
the channel `default` -- the right answer for a single-surface colony, the wrong one for
a bot with many chats. Whatever a surface calls "the same conversation partner" goes in
there: a Telegram/Slack `hop.chat_id`, a room, a phone number.

**The round belongs on the same edge.** One talky serves one round -- a change of the
participant set ends the generation and a new talky takes over (ADR-0002 E8) -- so the
ingress door is where `context.audience_set` is declared, as a JSON list in affinity
vocabulary. The keeper writes it onto the generation row at the open, the close carries it
back out on the write port, and a `memory-drain` on that port refuses a batch that has
none. Leave it out and the closed sessions of this talky do not reach a memory; nothing
anywhere on the path invents one (GH #273).

**Numbers on the hop need `int()`.** A proxy delivers JSON integers, CEL deserialises
them as `uint`, and a bare `hop.user_id == 12345` is silently **false** -- no error, no
log line. Every numeric condition on the ingress edge carries the cast.

**The reply lane carries three sorts.** A real answer; a round that hit
`max_iter`, marked `hop.round_capped == "1"` and -- since `collector@3.5.0` --
`hop.partial == "1"`, whose last turn is a **named partial answer** rather than the raw
end of the tool round ([#570](https://github.com/mmeyerlein/meclaw/issues/570)); and
-- since `collector@2.1.1` -- a turn that could not be assembled at all because the store
refused a read or a write, marked `hop.degraded == "1"` with `hop.store_error` and
`hop.store_operation` beside it
([#343](https://github.com/mmeyerlein/meclaw/issues/343)). The third sort carries **no**
`round_capped`, so a guard written against that key alone lets a failure through as a
real reply -- which is why the example edge above tests both.

**A real answer carries NEITHER key, and `!has()` is therefore the right test.**
`round_capped` and `partial` are written by the **seam** -- the message the assembler
builds for the brain, and the capped exit that leaves on `answer` instead of asking again.
A real answer does not come through the seam at all: it arrives on `in_answer` and leaves
on `answer` with nothing but the routing keys. So on the reply lane the three sorts read
as: a real answer has neither key, a capped round has `round_capped == "1"` **and**
`partial == "1"`, and a degraded turn has `degraded == "1"` and neither of the other two.

A paragraph here claimed the opposite from `talky@4.5.1` up to `4.6.0` -- that the
assembler stamps `round_capped` on every answer, so the guard had to read
`has(hop.round_capped) && hop.round_capped == '0'`. It does not, and that guard matches
nothing: measured against the shipped script, and against the two composite tests that
deliver a reply through this edge
(`talky_composite.rs`, `gh273_a_swept_close_reaches_the_memory.rs`), both of which use
`!has(hop.round_capped)`. Repaired with #570.

The composite does not decide which of the three a user sees: guard the reply edge with
`!has(hop.round_capped) && !has(hop.degraded)` and give each of the other two its own
edge (the error drain is the usual target for both) -- or let them through deliberately.
`hop.partial` is what tells the capped sort apart from a degraded one if you want to
render it rather than drain it: since `collector@3.5.0` it carries a readable sentence.

### Per-instance lanes (not lanes of this template)

**Tools stay outside.** The tool set is the per-agent choice, so the composite carries no
tool cells and no map of them. Wiring a tool is one edge pair:

```json
{"from": "./talky", "to": "./search",
 "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'web_search'"},
{"from": "./search", "to": "./talky",
 "modifier": {"set_hop": {"route": "'in_tool'"}}}
```

**Both halves of that condition matter.** `hop.route == 'tool'` is the lane -- it is what
tells a tool call apart from the `answer`, `write` and `error` traffic that leaves on the
same address -- and the `has()` guards are not decoration: an emission that carries no
`tool_name` at all makes an unguarded comparison **error** in CEL, which skips the edge
with a log line per lane per message. A tool name nobody answers to dead-letters and
stalls that round until the collector's idle window closes it (`round_idle_ms`).

### The two tools the composite serves itself

**Since `talky@4.2.2` the composite draws these edges, not the parent**
([#55](https://github.com/mmeyerlein/meclaw/issues/55)). `memory_recall` (GH #78) and
`thread_recall` ([#245](https://github.com/mmeyerlein/meclaw/issues/245)) are not the
per-agent choice above: the collector inside this hive **serves** both of them. It holds
the recall port for the per-turn leg, so the memory tool is answered where the memory leg
already lives; and it owns the round table a `thread_recall` stub points at, so the way
back to an elided payload is a table lookup in the cell that elided it. Both are therefore
routed internally, by two ordinary edges of this template's own `params.graph`:

```json
{"from": "./dispatcher", "to": "./collector",
 "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'memory_recall'",
 "modifier": {"set_hop": {"route": "'in_memory_call'"}}}
```

… and the same shape one lane further for `hop.tool_name == 'thread_recall'` →
`in_thread_call`. Nothing downstream changed: the `collector` has accepted both lanes
since `2.0.1`, and the door edge from `.` still routes them for a caller that hands such a
call in from outside.

**RETRACTED: the parent no longer draws the two self-loops.** Up to `talky@4.1.1` this page
told every parent to wire each name as a loop at the composite's own address -- `{"from":
"./talky", "to": "./talky", "modifier": {"set_hop": {"route": "'in_memory_call'"}}}` -- and
the port table below carried a row for each. **That instruction is withdrawn.** A parent
that still draws them delivers every such call **twice**, once by its own edge and once by
the composite's, so remove them when you lift an instance to `4.2.0`. The `in_memory_call`
and `in_thread_call` lanes stay declared and stay real doors: a parent MAY still hand such
a call in, it simply no longer has to.

**The tool exit stopped naming names.** `./dispatcher -> .` on `hop.route == 'tool'` is a
**guarded default edge** since `4.2.0` ([#283](https://github.com/mmeyerlein/meclaw/issues/283),
ruling Q1): it is consulted only when no ordinary edge out of `./dispatcher` fired for the
message. The two edges above are ordinary, so a reserved name silences the exit for itself
and no term of exclusion is written anywhere. A third reserved name would cost one ordinary
edge and **no** change to the exit at all -- which is the whole difference between a default
edge and the negation chain it replaces.

**Two properties keep that honest, and both are measured against this tree rather than
assumed.** The guard is not decoration: `./dispatcher` emits four sorts (`calls`, `result`,
`answer`, `tool`) and default suppression is **sender-wide**, so an unguarded default would
try to carry `calls`/`result`/`answer` outward whenever nothing ordinary happened to fire
for them. And there is **no unconditional tee**: `./dispatcher` has six out-edges here and
every ordinary one is conditioned on its own lane. If you add a tee of your own -- a logger,
a tap, a mirror, at `./talky/dispatcher` -- condition it on its own routes, or it silences
this default for every tool call and your tool cells go dark.

**RETRACTED: the composite carries the schemas of the two tools it serves.** Up to
`talky@4.1.1` this page said: *"The tool SCHEMAS are a different thing again: they live in
the brain's `system.tools`, seeded (`brain/seed/system.jsonl`) or written by a system
update. The composite carries neither -- identity, instructions and tools are the agent,
not the topology."* The second half of that no longer holds, and the line runs elsewhere
now: **a tool the composite implements is topology; a tool the parent wires is the agent.**
So the composite carries the schemas of `memory_recall` and `thread_recall` -- schema and
edge together, because a schema without the edge is a call into a void and an edge without
the schema is a tool the model never learns it has -- and it carries **no others**.
Identity, instructions and every per-agent tool stay the agent's, in the brain's own
`cell.db`. The schema half of #55 ships in the brain's seed (`brain/seed/system.jsonl`);
the `memory_recall` parameter block is documented in
[`../collector/README.md`](../collector/README.md) § The memory tool.

**The seed takes on a FRESH birth only, and saying that is half the promise.**
`brain/seed/system.jsonl` is loaded by `LlmCellFactory` at spawn and **only** when the
brain's `cell.db` was created in that moment (`OpenStatus::Created`); a resume never
re-seeds, because a template default that overwrote the accumulated identity at every
restart would be worse than no seed at all (`docs/cell-types.md` § Seed,
`crates/meclaw-cells/src/llm/seed.rs`). So an **existing** talky does **not** gain the two
tool schemas from a template bump. It gains them one of two ways: a `system.tools` update
message -- a body carrying `system` and no `messages[]`, which upserts the slot and
triggers no inference -- or a fresh generation whose brain starts with a new `cell.db`.
Bumping the template and expecting a running agent to change is the half-truth that
produces "but it works in a new colony" bug reports; the edges of #55 travel with the
mutation, the schemas do not.

### The sentence a memory-carrying persona has to contain

Whatever else an agent's identity says, one boundary is topology-invariant and belongs in
its instructions **verbatim**:

> What stands in this conversation window is your own knowledge, not something to look up.
> A question about what was just said you answer immediately, with no tool and no bridging
> sentence. The core / the memory is for what is **not** in the window.

Without it the front model asks memory for what it was handed a moment ago (#150, measured
in production): the answer is *correct*, so nothing looks broken — it just cost a bridging
sentence, a consult round trip and about six seconds instead of one and a half. The
instructions of a persona naturally enumerate what the core is FOR (deep thinking,
research, planning, long-term memory) and, without this sentence, never say what it is not
for. "What did I just tell you" reads, literally, as a memory question.

The same sentence is what keeps the boundary honest in the other direction: it names the
window as the model's own knowledge **and** the long-term store as the thing it must ask
for, so a question about an earlier day still leaves through the lane it should.

Eight more lanes -- six of them the parent's decision, and two (the memory tool and the
thread tool) drawn by the composite itself since `4.2.0`. Every one of them lives
**at the composite's own address**. `talky` declares `params.ports: []`, so a **runtime
mutation** may name no endpoint but `./talky`: an `add_edges` naming `./collector`,
`./session-keeper` or `./talky/dispatcher` is refused with `hive_port_boundary`, and the
last of those spellings would not even be this template's frame.

**The seal guards the mutation path, not the boot.** Only the `add_edges` of a mutation
diff is checked; the birth topology is deliberately out of scope (ruling 2026-08-15,
`crates/meclaw-colony/src/mutation/port_boundary.rs`) -- whoever writes a parent's
`params.graph` has the whole tree in front of them, and that is authorship, not a breach.
So a parent template *can* wire a cell straight at `./talky/dispatcher` at boot, and one
shipped test does exactly that. It has a consequence you have to carry: such an edge is an
ordinary out-edge of that sender, so it counts in the sender-wide suppression below. Do not
read the seal as a promise that nothing outside can ever reach a cell in here -- it is a
promise about what a later mutation, possibly written by a model, may reach into.

Which cell inside picks the lane up is
the door edges' business, exactly as it is for the four essential lanes:

| lane | endpoint | when |
|---|---|---|
| memory recall | `./talky` route `recall` out, lane `in_bundle` in | the per-turn leg only with `memory_tier` set; the same pair also serves the memory **tool** below |
| memory tool | **nothing to wire** -- the composite routes `hop.tool_name == 'memory_recall'` to its own `in_memory_call` lane | GH #78 / GH #55. Since `4.2.0` this row is a statement, not an instruction: draw the old self-loop and the call arrives twice. The recall pair above is still the parent's, because the answer comes from a memory hive |
| thread tool | **nothing to wire** -- the composite routes `hop.tool_name == 'thread_recall'` to its own `in_thread_call` lane | GH #245 / GH #55. Since `4.2.0` likewise. It matters most as soon as `context_window` is set, because from then on the curator leaves stubs that name this tool as their way back -- and now they always reach it |
| forced sweep | `./talky` lane `in_sweep` | an operator or a second schedule |
| housekeeping | `./talky` lanes `in_prune`, `in_round_sweep` | a timer; the template never fires them itself |
| per-turn write | `./talky` route `turn_write` out | one message per turn into a memory hive's `in_episode` lane; on by default -- see below |
| memory lookup | **nothing to wire any more -- RETRACTED in `4.5.1`** (GH #530). The `ask_memory` errand is retired; a fast memory question is the `memory_recall` row two lines up, answered by this composite out of its own recall port | the row used to send that question to a core that has no memory leg. See *The one errand, and the memory question that is not one* |
| inline extraction | `./talky` on `hop.route == 'extraction'` -> the memory hive's `in_remember` lane, **plus** its `reject` egress into the parent's own drain | the memory's write path for what a turn carried -- see "The extraction sidecar". Since `talky@4.1.0` the lane is a ROUTE, not a tool name: a parent still wired on `hop.tool_name == 'remember'` writes nothing |

### Per-turn episodes (`turn_write`)

The write port fires at the **close**. For a day archive that is right; for a memory it
means nothing said today is retrievable until the night sweep has run, so a question
about the last exchange is answered out of an empty store. The `turn_write` lane closes
that hole, and since GH #298 (ruling Q11) it is the **only** path from this conversation
into an episodes table -- which is why it is on by default. No model call is involved:
this is the collector's own table leaving one turn earlier.

What leaves is **one message per turn**, never a batch: one `user`/`assistant` turn in
`messages[]`, with `hop.turn_id` = `<session_id>#<index>`, `hop.turn_index` and
`hop.happened_at` beside it. That is the shape a memory hive's `in_episode` lane reads, so
the route is wired straight at the hive -- no decomposer in between:

```json
{"from": "./talky", "to": "./memory/keep",
 "condition": "has(hop.route) && hop.route == 'turn_write'",
 "modifier": {"set_hop": {"route": "'in_episode'"},
              "set_context": {"session_id": "hop.session_id",
                              "turn_id": "hop.turn_id",
                              "happened_at": "hop.happened_at"}}}
```

**Whoever wires the parent, wires this.** A `talky` inside a shipped `member` gets
it for free since [#527](https://github.com/mmeyerlein/meclaw/issues/527) — the member
draws exactly this edge from its own `./assistants` container to its own `./memory-hive`,
and drew none until then, so every colony grown from the library emitted the lane into a
`hive_no_route` at the OS root. A hand-drawn topology still owes it, and the edge above is
the one to copy: **`turn_id` off the HOP**. `context.turn_id` is a round uuid, `hop.turn_id`
is the deterministic id this cell mints, and an edge that promotes the context key writes
episodes the inline bind can never find.

**The `write` route is not a second half of this.** It carries a closed session with its
`rounds` slot, for whoever archives a day; wiring it into the same memory would be a
second writer over turns this lane already wrote. The two documents are different on
purpose (GH #298) -- and whoever consumes `write` for something else (a day archive, the
memory hive's close pass) is untouched, because `turn_write` is a route of its own
precisely so that the close-only consumers stay close-only.

**Delivered twice is written once.** Idempotence lives in the collector's own `turns`
table (`episode_written`), set by a guarded update that rides in the same emission as the
episode it covers, and the `turn_id` is deterministic -- so a repeat is recognisable
downstream as well.

## The internal wiring, edge by edge

Fifteen edges of round in this hive's `params.graph` -- plus the sixteen that ARE the
boundary (five door edges from `.`, eleven leaving towards it, and those are the lanes
above). The two halves are the whole of this file, counted from it. Every one of the
fifteen names a sub-unit **by its path**: two of the six nodes below are sealed hives, so
the address is the hive and the lane in the third column is what the door behind it
reads; what those two draw INSIDE themselves is theirs and is not counted here. Read it
as the round it is:

```
session-keeper --(turn, session_id -> context)-->  collector   in_turn
session-keeper --(close, session_id + channel + audience_set -> context)->  collector   in_close

collector ==(brain, int(hop.iter) < 12, restore_ttl)==>  brain      <- THE SEAM
collector --(pack)--------------> brain      <- THE DOOR IN THE WALL, GH #458
collector --(menu)--------------> brain      <- the answered tool menu, GH #464
brain --(stop | tool_calls)------> splitter      <- the sidecar cut, GH #379
splitter --(stop | tool_calls)---> dispatcher
splitter --(extraction)---------->  .            <- and out of the extraction port
brain --(length)-----------------> collector    in_answer
brain --(error | content_filter)-> errors
session-keeper --(reject)--------> errors    <- the session store refused a step

dispatcher --(calls)---> collector   in_calls    dispatcher ==(tool, DEFAULT)==> [your tools]
dispatcher --(result)--> collector   in_tool
dispatcher --(answer)--> collector   in_answer
dispatcher --(tool_name == memory_recall)--> collector  in_memory_call   <- served here
dispatcher --(tool_name == thread_recall)--> collector  in_thread_call   <- served here

collector --(write)---------->  .            <- the close batch, out of the write port
collector --(pack_ack)-------->  .            <- the pack receipt, GH #458
collector --(schemas)--------->  .            <- what tools this agent declares, GH #464

[sealed]  session-keeper  collector   [plain]  brain  splitter  dispatcher  errors
```

**The sixteen are not drawn here, and that is the point.** A sealed sub-unit takes its
lane at its own `{"from": "."}` door edges and distributes behind them -- `session-keeper`
alone brings eleven edges, `collector` five -- and none of that is
visible to, or wireable by, the hive above. What the fifteen edges above state is the
whole of talky's own topology.

**The one `==` in the fan-out block is the default edge.** `dispatcher --(tool)--> [your
tools]` is consulted only after the two `tool_name` edges below it have declined, which is
what lets them be written as plain positive conditions with no exclusion anywhere.

**The loopback bound is an edge literal, on purpose.** `int(hop.iter) < 12` is a safety
belt, not the policy: the round is bounded by `max_iter` (default 8), which
ends a runaway round with a message on the `answer` lane instead of a silence. The edge
number only has to be larger. Env substitution does not reach edge conditions -- a
`${VAR}` there would be registered verbatim and fail to parse as CEL -- so raising it is
a mutation: `remove_edges` first, `add_edges` second, in **two** mutations. A remove and
an add of the same endpoints in ONE diff match over the post-state and take the new edge
with them.

**`restore_ttl` sits on the seam, once per round.** `iter` counts brain answers, and a
bundle of fifteen calls is one answer, one iteration, one restore. The substrate refuses
a restoring edge without a condition, because the iteration bound is then the only thing
left stopping the loop.

### The door in the wall (`in_pack`, GH #458)

This composite is **sealed**, and that seal was, until 4.4.0, complete in a way nobody
had meant it to be: an edge naming `./brain` is refused with `hive_port_boundary`, the
only path from outside to the brain runs `.` -> `./collector` -> `./brain`, and the
collector drops `system.*` on every lane that could have carried one. So a shipped agent
had no entrance for its own identity at all. `affinity` could push -- `./push` hashed
what a subscriber would get, `./brief` rendered the pack, `./clock` ticked it -- into
nowhere, and the `subscribers` seed named a brain no graph could reach.

`in_pack` is that entrance, and it is one lane rather than a flag on an existing one for
the reason the collector's `in_tool` lane already spells out: what leaves the seam in
`system.*` is UPSERTed into the brain's own `cell.db` and stands until something
overwrites **that exact path**. It is durable state of the agent, not evidence of one
round -- right for an identity, wrong for a tool result -- and telling the two apart by
the LANE means the distinction is drawn by an edge the colony wrote, never by a key in a
body a model could have written.

**What may be written is a closed list**: `identity`, `persona`, `handover`,
`instructions`. That is a subset of the collector's `SYS_KEEP` by construction and not by
taste -- minus the two families the collector re-derives every round (`tools`, `budget`),
because a sender writing those would fight it for the same path forever. What remains is
exactly the durable, curator-proof, unowned four. An unknown slot refuses the **whole**
pack with `slot_unknown` and an empty one with `pack_empty`: writing the half that was
understood would hand the sender an `ok` it cannot tell apart from a complete write, and a
half-written identity is worse than none.

`instructions` was a third subtraction until GH #488, on the ground that an identity able
to overwrite the charter could rewrite what the agent is for. That ground assumed the
charter had some OTHER owner, and it had none: nothing exported it, no template seeded it,
and a rebuilt generation came up with an empty charter and answered as the vendor's default
assistant. A family nobody may write is not a protected family, it is an empty one. What
guards it now is the lane rather than a hole in the lane -- the door is a route stamped by
an EDGE, and that edge is drawn only through the access rule that lets a brain draw its OWN
push edge, from a source whose single writer is `affinity`'s audited gate.

**Two body shapes, one meaning.** `system` carrying the slot subtrees is what an `llm`
cell upserts and what `affinity`'s push lane already emits -- the slots and **no** turn
beside them, so the update costs the agent a write and not an inference (GH #263).
`{"slot": ..., "content": ...}` is the same thing for a single slot, for a caller writing
one by hand. Both may travel in one message; the single slot is merged over the tree. The
single-slot form is a convenience and not a body of its own: a UBF body must carry
`messages` or `system` -- the substrate's `anyOf`, not this template's rule -- so it rides
beside an empty `system`, `{"system": {}, "slot": "persona", "content": {...}}`, and is
dead-lettered as `invalid_ubf_body` without it.

**The owner comes off the envelope.** `envelope.reply_to`, never a name in the body --
the rule `affinity`'s gate states for `context.actor` and the `display` hive states for a
view's owner, and for the same reason: bodies are written by whatever produced the
message, up to and including a model.

**`in_pack` and `pack_ack` are one decision**, paired in `params.required_drains` exactly
like the prune. The receipt answers unconditionally, because from the sending side a push
that landed and a push that reached nothing are the same silence otherwise.

```json
{"from": "<member>/affinity", "to": "./talky",
 "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber == './talky'",
 "modifier": {"set_hop": {"route": "'in_pack'"}}},
{"from": "./talky", "to": "<log or drain>",
 "condition": "has(hop.route) && hop.route == 'pack_ack'"}
```

The source is the affinity **hive** path and not `<...>/affinity/push`: affinity's
`params.ports` is empty too, so the hive path is its only endpoint and an edge reaching
into `./push` is refused by the same rule that protects `./brain` here.

**Drawing that first edge is a formal act.** It is a mutation like any other -- through
`tools/build-apply`, the operator, `submit`, the gate and the broker -- and the gate recognises
it by its form: an `add_edges` entry whose `modifier.set_hop.route` is `'in_pack'` opens
a door into somebody's prompt, so the gate checks that the door is the requester's **own**
(the target is the requester's hive, the source is an `affinity`) and the broker answers
the separate question of whether this identity may subscribe at all, under the capability
`affinity.subscribe`. Which side checks what is the contract between the two templates:
a policy row cannot compare two fields of one request against each other, so the FORM
cannot live there, and a capability that could be granted for "any `in_pack` edge" would
let one agent open a door into another agent's prompt.

#### The sentence a subscribing agent's instructions have to contain

The composite ships no persona and no instructions -- `brain/seed/system.jsonl` carries
the two tool schemas the composite serves itself and nothing else, and that boundary is
pinned. So the brief that makes an agent subscribe belongs to the **instance**, verbatim,
beside the memory sentence above:

> Your identity is not written here. It is a record your member keeps, and it reaches
> this prompt as `system.identity` only once you have subscribed to it. Subscribe once,
> at first boot, and never again: one mutation drawing the push edge from the record into
> your own `in_pack` lane, then one `subscribe` naming yourself. If you have no tool that
> submits a mutation, you cannot subscribe and this paragraph is not about you.

The last sentence is not padding. A talky is instantiated in trees that wire no operator
stack at all, and an instruction that told such a model to do something it has no lane for
would buy a tool call into a void once per boot, forever.

**The order is edge first, row second**, and it is chosen so that the half-finished state
is the harmless one: an edge with no active subscription behind it carries nothing, because
`affinity`'s push selects on `status = 'active'` and never sees the subscriber. A row with
no edge behind it is the opposite -- accepted, written and silently undeliverable, which is
the failure [#289](https://github.com/mmeyerlein/meclaw/issues/289) named and could not
refuse. Nobody mints a token for either half: `hop.subscriber` carries the subscription
row's own `cell_path`, so the subscriber's address **is** the token and both halves can
name it before either has run.

### The menu is asked for, not typed (`schemas` / `in_menu`, GH #464)

Until 4.5.0 a talky's tool declarations were a list somebody had written into its brain's
`system.tools`, or into the collector's `tool_menu`. Adding a tool to a colony meant editing
that list in every agent that might use it, and no agent could offer a model anything nobody
had typed.

**What the composite owns is the DECLARATION.** `./collector`'s `params.tools` names the
tools this agent uses -- shipped as `["web_search", "web_fetch"]`, `["*"]` for everything a
tools hive has -- and the schemas behind those names are asked for:

```json
{"add_nodes": [{"name": "scribe", "template": "talky@4.6.1",
                "override_params": {"collector/assemble": {"tools": ["web_search", "bash"]}}}]}
```

**The lane pair, and it is a pair.** `schemas` leaves the composite with `{"tools": [...]}`
as its whole body; `in_menu` brings `schemas[]` and the names the hive did not have back.
Drawn against a `tools` hive standing beside this one, that is two edges and never one --
the same shape as the tool lanes, and refused half-drawn by that hive's own
`params.required_drains`:

```json
[
  { "from": "./talky", "to": "./tools",
    "condition": "has(hop.route) && hop.route == 'schemas'",
    "modifier": { "set_hop": { "route": "'in_schemas'" } } },
  { "from": "./tools", "to": "./talky",
    "condition": "has(hop.route) && hop.route == 'tool_schemas'",
    "modifier": { "set_hop": { "route": "'in_menu'" } } }
]
```

**What arrives is durable, and that is why it has a lane of its own.** The collector wraps
each declaration in the provider envelope -- the hive answers provider-neutral on purpose,
because a hive that wrapped would have to be told which provider its caller talks to -- and
writes the result into `./brain` as `system.tools`, with no turn beside it. An `llm` cell
upserts `system.*` per slot path into its own `cell.db`, so the menu costs **one write per
change and nothing per turn**. It is the same door `in_pack` uses one section down, and it
is durable for the same reason.

**The tick is not a birth.** The substrate hands a cell no message at spawn, so nothing can
ask at boot: the first ask is the first firing of `./collector/menu-clock`, whose cadence is
`MENU_CRON`. That is also how a tool ADDED to the hive later reaches this agent -- nothing
over there pushes. An operator who does not want to wait triggers the schedule by id.

**A name nobody has is named.** It comes back in `hop.menu_unknown` and as a warn line in
`log.jsonl`; a declaration pointing at nothing is a defect in this agent's own template, and
the value of declaring is that it is visible. And a collector carrying a typed `tool_menu`
asks nothing at all -- that knob is the manual override, not a second source.

**Since `3.4.0` the collector MERGES the answers of several answerers**
([#529](https://github.com/mmeyerlein/meclaw/issues/529)). One tick asks every answerer
the tree wires at once, and each reply says who sent it in `context.tool_answerer` -- the
mirror of the `context.tool_caller` the request already carries. The collector keeps each
answerer's last submenu as one row of its own store, keyed by that name, and on every reply
writes the UNION of the rows plus the names it serves itself, with `$replace`. Before the
merge one answer *was* the menu, which is right while exactly one thing answers and is the
whole defect the moment two do: the second reply would not join the first, it would delete
it, and the two would take the menu away from each other on every tick forever. **A reply
with no `tool_answerer` counts as the default answerer**, so a tree wired the old way
behaves exactly as it did.

`hop.menu_unknown` moved with it: it is computed against the MERGED menu, so a name one
answerer has nothing under is not a finding when another answerer delivers it.

**This composite still asks exactly one answerer, and that is deliberate.** Standalone, a
talky has a tools hive beside it and nothing else that could serve a tool name, so its
shipped declaration is its two search tools. The declared list grows **one level up**: in
[`../assistant/README.md`](../assistant/README.md) the level adds `consult_cogny` to it and
draws the `schemas` / `in_menu` pair a second time, out to the reasoning core -- because
`consult_cogny` is not a tool of any hive, it is an errand that LEVEL routes, so only the
side that answers it can declare it. That is the level's decision for the same reason the
model is (GH #516), and neither the list nor the second pair belongs in this template.

The full account of the mechanism lives in
[`templates/collector/README.md`](../collector/README.md) § *The menu is asked for*, and the
answering side in [`templates/tools/README.md`](../tools/README.md) § *Asking for the
declarations*.

### The advisor lanes (GH #28, R-CG-3)

An agent core (`cogny`) is a **sibling hive** of the talkies, not a cell inside one: one
core, N channel voices. It is reached like a tool and answers like an event, so the
connection is two edges plus one knob.

```json
"DISPATCHER_HANDOFF_TOOLS": "consult_cogny"
```

A consult is a **handoff**, not merely async: the advisor's answer comes back as its own
turn on `in_advice`, so the round the call leaves behind is over even when the model sent
no sentence beside it. Naming a tool in the handoff list declares it async as well -- the
dispatcher unions the two ([#372](https://github.com/mmeyerlein/meclaw/issues/372)).

```json
{"from": "./talky", "to": "/front/cogny",
 "condition": "has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"consult_id": "hop.consult_id", "col_phase": "''"},
              "restore_ttl": true}},
{"from": "/front/cogny", "to": "./talky",
 "condition": "has(hop.route) && hop.route == 'answer'",
 "modifier": {"set_hop": {"route": "'in_advice'"},
              "set_context": {"col_phase": "''"},
              "restore_ttl": true}}
```

Four things in that pair are load-bearing:

- **`col_phase` must be cleared.** Both messages leave *another* collector's chain and
  carry whatever step that chain was in. A collector's `in_turn` / `in_advice` refuses a
  message that arrives mid-assembly, so the port edge resets the key. Everything else in
  the context rides along on purpose -- `session_id` above all, which is what keeps one
  consultation inside the channel's own session.
- **`consult_id` becomes context**, because the hop is single-hop and the correlation has
  to survive the core's whole chain and come home with the answer.
- **`restore_ttl` on both**, with the condition they already carry: an errand is a fresh
  journey, not the tail of the turn that started it.
- **The errand arrives as a `tool_call` turn.** Its text is the raw arguments the model
  wrote, and the core's collector files that as the turn: the talky IS the core's user.

What the parent does *not* wire: nothing else. The turn ends with the interim answer the
dispatcher already sent to the channel, and the returning advice starts a fresh talky
round that verbalises it in the channel's own voice.

**The duration estimate (GH #123, observe-only).** Put the hints in the brain's own
instructions and let the model fill `arguments.eta` in the same call it already makes:

```
consult_cogny(question, context, eta): eta is a coarse guess at how long the
answer will take -- "about ten seconds" for a short one, "half a minute" for
real reasoning, "a minute or more" once a web search is involved. Say what you
are doing in the same reply; that sentence reaches the user immediately.
```

Those three phrasings are **measured, not chosen** (GH #124). Read out of a running
colony's own message log with [`scripts/trace_latency.py`](../../scripts/trace_latency.py),
the lookup lane sits around ten seconds and the thinking lane spreads much wider, with
its single largest gap being the core model's own turn. The earlier wording -- "seconds"
for a lookup, "a minute" for a search -- was a guess, and it was optimistic in the
direction that costs trust: a user told "seconds" who waits eleven of them has been
misled by the system, not by the model.

Measure your own deployment before you copy these words. The tool needs nothing but the
colony root, costs nothing, and takes a second:

```
scripts/trace_latency.py <colony-root> --lane brain_fast --lane brain --breakdown
```

The estimate still rides out as `hop.consult_eta` and **nothing reads it**. Consuming it
-- routing by expected class rather than by tool name -- is the open half of #124.

**RETRACTED: the second errand name `ask_memory`** (GH #124, cogny 1.1.0 --
retired in [#530](https://github.com/mmeyerlein/meclaw/issues/530)). Up to `4.5.0` the
advisor lane carried TWO errand tools and the core's ingress edge turned the model's
choice between them into `context.consult_class` -- `'consult'` for `consult_cogny`,
`'lookup'` for `ask_memory` -- which is how the core picked its fast brain. **That edge is
gone, that name is gone, and this is a retraction and not a rewording:** an instance whose
charter still offers `ask_memory` offers a tool no edge carries, and the call is delivered
nowhere.

Why it had to go rather than be renamed: the lookup class assumed the core could answer a
memory question, and at the time it was drawn **the core had no memory leg at all** --
`cogny` ships `memory_tier` empty, so nothing assembled a bundle for a turn of the core,
and an `ask_memory` errand reached a brain that answered a question about memory without
one. The surface HAS one, one hop away: its collector serves `memory_recall` itself, out
of its own recall port, against the person's memory.

**And the boundary outlives that reason.** Even where a tree wires the core a memory leg
of its own, a lookup routed through the core is a whole extra TURN -- the errand leaves,
the round it left behind ends, and the answer comes home later to be said again in the
surface's voice -- against one tool call answered inside the round the person is waiting
in. The class boundary is that difference, not the question's subject matter.

**The one errand, and the memory question that is not one.** So the boundary is no longer
a name the model picks between two errands; it is the line between two MECHANISMS, and
only one of them leaves this composite:

| the question | who answers it | how it comes back |
|---|---|---|
| a fast memory question -- one lookup, a flat tier, a second or two | **this composite**, through its own `memory_recall` tool (GH #78 / GH #55) | inside the same round, as a tool result |
| synthesis, a time series, anything multi-step or research-shaped | **the core**, through `consult_cogny` | as its own later turn on `in_advice` |

**The errand takes `question` AND `context`, both required, and the asking side filters
NOTHING.** It does not drop what it believes it has already sent. The answering side
curates -- it is the side that knows what its own window holds -- and a filter on the
asking side would be a second curator working with less information, which is the shape
that loses the one paragraph the answer needed.

**`session_id` travels as persistent context and MUST NOT be promoted on the edge.** The
keeper stamped it on the context at the start of the turn, `context` is persistent while
`hop` is single-hop ([#521](https://github.com/mmeyerlein/meclaw/issues/521)), and nothing
between there and the core deletes it -- so it is already on the errand. Writing
`"session_id": "hop.session_id"` into the edge modifier does not merely repeat it: a
dispatcher's tool emission carries no `hop.session_id`, the modifier FAILS, and **a failed
modifier skips the edge** -- which silently kills every consult the agent ever makes.

**The charter paragraph, to copy.** This is the level's own promise about the errand, not
talky's, so it belongs in the instance's `instructions.reply` slot. Copy it as it stands
and change only the tool names your tree actually wires:

```text
You have a reasoning core beside you. Ask it with consult_cogny(question,
context) -- both arguments are required. `question` is what you want answered.
`context` is what the core needs in order to answer it: who is asking, what was
already said, what you already know. Do not leave anything out because you think
you sent it before -- you did not, and the core cannot ask.

The answer does not come back inside this reply. It arrives later, as its own
turn, and you will say it then in your own voice. So say in THIS reply what you
are doing and roughly how long it will take.

Do NOT use consult_cogny for a plain memory question. "What is my mother's
name", "what did we decide about the invoice", "what do we know about X" -- ask
memory_recall yourself; it answers in this same round, in about a second, out of
your own memory. Send consult_cogny the questions that need thinking: a
synthesis, a comparison over time, anything that takes several steps or has to
be looked up outside.
```

Everything the core does with the errand -- what it assembles, which brain it runs and how
the answer leaves -- is in [`../cogny/README.md`](../cogny/README.md).

### The extraction sidecar (inline extraction)

The lanes above ASK the memory. This one WRITES to it, and it is the only lane on
which the brain does two jobs in one call: it answers, and in the same response it emits
the durable memory the turn carried. That saves a second inference over the whole window
-- but the reason to do it is freshness, not tokens: a fact extracted at night cannot
answer a question asked this afternoon.

**This used to be a TOOL, and that instruction is retracted rather than reworded**
(owner ruling 2026-08-24 on [#373](https://github.com/mmeyerlein/meclaw/issues/373),
built in [#379](https://github.com/mmeyerlein/meclaw/issues/379)). The block asked the
model to `call remember` after its answer; measured across seven model families, the best
case carried the call on 44 % of turns and most were far below that -- and a completion
that mixed a sentence with an asynchronous call stranded its own round
([#378](https://github.com/mmeyerlein/meclaw/issues/378), still open as a substrate item).
The same rules delivered as a fenced block INSIDE the answer were adopted on 12 of 12
turns by every one of five models, with zero malformed blocks. So since `talky@4.1.0` the
model writes the annotation into its own text, and a cell takes it back out again.

**The splitter, in one line.** `./splitter` sits between `./brain` and `./dispatcher` on
the answer path. A completion whose text carries a ```` ```memory ```` block leaves it as
TWO messages: the answer with the block cut out, on to the dispatcher exactly as before,
and the raw block on lane `extraction`, out of the composite. Everything else passes
untouched -- a round with tool calls belongs to the dispatcher whole, and **without the
extraction prompt the splitter is a pure pass-through**.

**A block it cannot read leaves the answer too, and that is a retraction**
([#534](https://github.com/mmeyerlein/meclaw/issues/534)). Until then an unreadable block
was left INSIDE the answer and only flagged: half-cutting a block nobody can read, the
reasoning went, corrupts the answer for the sake of a write that cannot happen anyway. It
was measured wrong in a running colony -- a model that had annotated the turn before it
correctly dropped one closing brace, and the raw JSON travelled through the dispatcher and
out to the chat window. There is no half cut to fear: the parser has already located the
span, and the prose either side of it is the same prose whether or not the JSON in the
middle parses. So **`found` decides the cut and `valid` decides the lane** -- an unreadable
block comes out of the answer, `hop.sidecar == "malformed"` records that one was seen, and
NOTHING goes out on `extraction`, because a block the hive cannot read is not an annotation
and repairing it would hand the store this cell's invention instead of the model's. An
opener with no closer is cut the same way, to the end of the text: cutting only the JSON
would leave the bare fence line standing, which is the same leak one character smaller. Which is why the prompt is
delivered by this composite and not by whoever writes an identity: `./collector` carries
the shipped block and writes it into the brain on every assembly (#525). Its own
`description` carries the rest; the grammar it cuts with is the one the harness measured
the wording with.

**It is the write path, not a write leg beside one.** Per-turn extraction
([#298](https://github.com/mmeyerlein/meclaw/issues/298)) removed the batched extractor
that used to read the same turns a second time, so what this tool does not emit, nothing
emits mid-conversation: the night is the reader behind it, and the close pass at the end of
the session is the second one -- that lane lands later in wave 5
([#300](https://github.com/mmeyerlein/meclaw/issues/300)), and until it does a turn nobody
annotated is read by nobody. That is why the annotation is an
**obligation on every turn** rather than an opportunity on the interesting ones -- a turn
nobody annotated is a turn nobody extracts, and the shipped contract block says so in its
first line.

```json
{"from": "./talky", "to": "/front/memory",
 "condition": "has(hop.route) && hop.route == 'extraction'",
 "modifier": {"set_hop": {"route": "'in_remember'"}}},
{"from": "/front/memory", "to": "<drain or alarm>",
 "condition": "has(hop.route) && hop.route == 'reject'"}
```

**Two edges, never one.** The first carries the annotation into the memory hive's `in_remember`
lane -- the hive seals its scope the same way this one does, so the address is the hive
and the lane is what the door behind it reads; the door stamps `store_origin` and
`mem_phase` itself, and what the lane does require of the caller (the block's provenance,
#244) is in [`../memory-hive/README.md`](../memory-hive/README.md) § Lanes. The second is
the hive's `reject` egress, and a hive egress nobody drains is an unrouted dead end -- a
block the hive discarded would vanish without a line anywhere, and the memory it was meant
to write would silently never exist.

**The reject goes to the parent's own drain, not back in here.** `./errors` is inside this
composite's seal and is not an address a parent may name (`hive_port_boundary`), and
`./talky` is an address that would take it nowhere: `reject` is not one of the lanes
`params.contract` accepts, so it matches no door edge and dead-letters as `HiveNoRoute`.
Send it wherever the `error` port already goes -- the parent drains one place, which was
the point of that port.

**The annotation costs the turn nothing, and it never was the round's business.** It leaves
on its own lane while the answer travels the dispatcher, so no fan-in expectation is opened
and no idle window is waited out. This is the part the sidecar made simple rather than
merely correct: there is no call to classify, no async list to remember to fill in, and no
completion that carries a sentence beside an asynchronous call -- which was the shape that
stranded rounds ([#378](https://github.com/mmeyerlein/meclaw/issues/378)).

**The session travels by itself, and it is load-bearing.** The seam edge promotes
`hop.session_id` into the context long before the answer exists, so the annotation arrives
at the hive carrying the conversation it was written in. That is what the hive binds the
block to -- the front model names no episode, because an episode id is a uuid the hive
mints and no model has ever seen one. An annotation that reaches the port without a
session in its context is rejected, by design.

#### The retracted tool form (historical)

Everything below this line describes how the lane worked until `talky@4.1.0`. It is kept
because the measurement harness can still run that arm
(`workshop/evals/conversation-guide/run_guide.py --annotation tool`) and because a colony
that has not been rewired yet still looks like this. **Do not wire it into anything new.**
The parent edge was `hop.tool_name == 'remember'` instead of `hop.route == 'extraction'`,
and the brain carried a `remember` tool named in `DISPATCHER_ASYNC_TOOLS` (never in
`DISPATCHER_HANDOFF_TOOLS`: a memory write answers nothing and never comes back, so the
model still owed the turn a sentence, and putting it in the handoff list brought back the
silence [#372](https://github.com/mmeyerlein/meclaw/issues/372) had fixed).

**The schema it carried, for the brain's `system.tools`.** Like every tool schema it is
instance state (`brain/seed/system.jsonl` or a system update), never template:

```json
{"type": "function", "function": {
  "name": "remember",
  "description": "<the contract block -- see below>",
  "parameters": {
    "type": "object",
    "properties": {
      "nothing_new": {"type": "boolean",
                      "description": "true when the turn carried no world state"},
      "facts": {"type": "array", "items": {
        "type": "object",
        "properties": {
          "subject": {"type": "string"},
          "predicate": {"type": "string",
                        "description": "snake_case English key, lower case, no spaces"},
          "claim": {"type": "string"},
          "fact_kind": {"type": "string", "enum": ["world", "experience", "foresight"]},
          "valid_from": {"type": "string"},
          "confidence": {"type": "integer", "minimum": 0, "maximum": 100}},
        "required": ["subject", "predicate", "claim", "fact_kind"]}},
      "topic": {"type": "object",
        "properties": {
          "movement": {"type": "string", "enum": ["start", "continue", "end"]},
          "name": {"type": "string"}},
        "required": ["movement"]}},
    "required": ["facts", "topic"]}}}
```

**Both parts are required, and that is the schema half of the obligation
([#299](https://github.com/mmeyerlein/meclaw/issues/299)).** `facts` is the delta of world
state the turn carried, `topic` is where the conversation stands -- a topic is not a fact
(it has no subject and no predicate), so it gets a part of its own rather than an axis
about the conversation next to the axes about the world. A turn that carried nothing
answers with an empty `facts` list, `movement: "continue"` and `nothing_new: true`; the
hive books that turn as *annotated and empty* instead of leaving it in the queue as one
nobody ever looked at. `nothing_new` is deliberately NOT required: the ingress reads it as
a flag that is either true or absent, and a `false` demanded on every content call would
be a field to get right on the turns where it means nothing. `name` is optional inside
`topic` for the same shape reason -- a `continue` writes no row, so it needs none.

**What is NOT in the schema is the point.** There is no `episode_id`, because the model
cannot know one and an invented id would file the facts against the wrong turn. And there
is no `valid_until`, because a validity a model derives from the range a QUESTION asked
about closes the fact on arrival -- invisible to the as-of leg, visible to keyword and
semantic, which is worse than a duplicate. Both were measured in a running colony. A
field a schema does not offer is a field constrained decoding cannot produce; the hive
enforces the same two rules again at its end, because it does not own the persona.

**The block IS the contract, and this composite DELIVERS it** (#525). The authority is
`templates/memory-hive/inline-contract.md`; what puts it in front of a model is
`./collector`, which writes it to the brain's `system.instructions.sidecar` on every
assembly, because `inline_extraction` is switched on in this composite's own
`collector/config.json`. Nothing has to be pasted anywhere, and that is the repair: the
instruction used to be *paste the fenced block into the brain's instructions*, nothing
shipped executed it, and a colony ran the splitter, this lane, the hive's ingress and both
required drains for weeks with `episodes` growing and `facts` standing still. A promise of
a TEMPLATE kept inside a person's charter is a promise every hand-written charter drops.

The block is short on purpose: it is carried on every single turn, so its length is paid
for once per call. The file stays the authority -- one drift lock
(`crates/meclaw-cells/tests/gh299_the_contract_asks_for_both_parts.rs`) holds the block to
what this lane can actually read, a second
(`crates/meclaw-cells/tests/gh525_a_grown_brain_carries_the_extraction_contract.rs`) holds
the collector's copy byte-identical to it -- and a discipline each persona invents for
itself is a discipline nothing can hold to account.

**Order the instructions so the answer is written first.** The block belongs AFTER the
answer, not beside it: a model that produces its structured field before its reasoning
answers from nothing, which is the one robust finding in the format-constraint
literature. The shipped contract says so in its first line -- and since #525 the ordering
is mechanical rather than advisory: an `llm` cell walks the leaves of a `system.*` family
in alphabetical order, so the collector writes the block to `instructions.sidecar`, which
sorts AFTER the charter in `instructions.reply`. That is the whole reason the slot is not
named after its own lane.

**It needs per-turn episodes.** A block is bound to the turn it answered, and that turn
has to BE in the memory when the call arrives. The knob is on by default since GH #298 --
leave it on and wire the `turn_write` lane above (a shipped `member` draws that edge
itself since #527); without that lane the hive has nothing to
bind to, rejects every block,
and the turns wait in the queue for the close pass at the end of the session. That is the
safe direction -- one extraction later is a delay, a fact hung on the wrong turn is a
defect, and only one of the two can be repaired -- and it is also the whole reason wave 9
came first.

## Knobs

Two classes since `collector@1.2.0`. The **env** knobs are `${VAR:-default}` literals that
travel into the instance and bind **late**, at every read, so a `.env` change plus a reboot
moves them without touching a config -- and they move every unit in the colony at once. The
**param** knobs ship with their defaults inside `./collector/assemble/config.json` -- the
hive marker one level up (`./collector/config.json`) carries `ports`, `contract` and
`graph` and no knob at all, so a value written *there* is read by nothing and the instance
comes up unconfigured with no diagnostic anywhere
([#212](https://github.com/mmeyerlein/meclaw/issues/212)) -- and are
retuned per instance with `override_params` on `collector/assemble.params.<name>`, so this
talky can differ from the cogny next to it. That deep key is **not** an edge endpoint and
the port boundary does not apply to it: `override_params` is addressed by the cell's path
inside the template (GH #140), which is how a sealed sub-unit stays tunable at birth while
being unwireable from outside.

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

| knob | where | default | unit |
|---|---|---|---|
| `KEEPER_IDLE_MS` | env | `7200000` | session-keeper -- silence before a generation may end (2 h) |
| `KEEPER_NIGHT_CRON` | env | `0 0,30 22-23,0-3 * * *` | session-keeper -- the sweep, **in UTC** (summer image of 00:00-05:30 CEST) |
| `KEEPER_CLOSE_LIMIT` | env | `50` | session-keeper -- generations one firing may seal |
| `window_turns` | param | `12` | collector -- newest turns entering the context |
| `window_bytes` | param | `8000` | collector -- byte cap over the window |
| `turn_chars` | param | `4000` | collector -- per-turn cap before the byte cap |
| `tool_chars` | param | `4000` | collector -- per-item cap on tool results |
| `round_bytes` | param | `16000` | collector -- byte cap over the whole tool round |
| `memory_chars` | param | `8000` | collector -- cap on the memory bundle |
| `max_iter` | param | `8` | collector -- **the loop bound**; at the cap the seam leaves on `answer` |
| `round_idle_ms` | param | `120000` | collector -- idle window of one tool round |
| `memory_tier` | param | `""` | collector -- empty = no memory leg at all |
| `memory_call_tier` | param | `"1"` | collector -- tier of the `memory_recall` tool; empty = tool off |
| `memory_form` | param | `"readable"` | collector -- `readable` / `json` / `both` |
| `prune_after_ms` | param | `604800000` | collector -- age gate on the prune lane (7 d) |
| `turn_write` | param | `"1"` | collector -- **on by default** (GH #298): one message per unwritten turn leaves on route `turn_write` after every stored turn and every stored answer. `""` or `"0"` switch it off, and off means nothing said in this session reaches a memory at all |
| `context_window` | param | `0` | collector -- the curator's budget in tokens; `0` = curation off. A channel voice is the shape the curator was **not** built for; leave it off unless the window is genuinely large. The full curator table is in [`collector`](../collector/#knobs) |
| `tools` | param | `["web_search", "web_fetch"]` | collector -- the tool names this agent **declares** it uses (GH #464). Set at this template since `4.5.0`: a channel voice wants a small, named surface, so the shipped list is two tools and not `["*"]`. The schemas behind the names are asked for on the `schemas` lane and written into the brain as `system.tools`; an empty list asks nothing at all. A level that puts a reasoning core beside this surface overrides the list to add `consult_cogny` (GH #529) -- the errand is the level's, not this template's. See [The menu is asked for](#the-menu-is-asked-for-not-typed-schemas--in_menu-gh-464) |
| `cron` | env | `0 */5 * * * *` | collector/menu-clock -- `MENU_CRON`, the cadence of the menu tick (UTC). The first ask is the first tick, because the substrate hands a cell no message at spawn |
| `DISPATCHER_MAX_CALLS` | env | `16` | dispatcher -- per-answer call budget |
| `DISPATCHER_ASYNC_TOOLS` | env | (empty) | dispatcher -- comma-separated tools that answer on their own lane instead of inside the round. The key is colony-global, so in practice ONE list carries every async name of the tree. It carried `remember` until `talky@4.1.0`; per-turn extraction is not a tool call any more (GH #379), so the list is empty unless the instance wires an async tool of its own |
| `DISPATCHER_HANDOFF_TOOLS` | env | (empty) | dispatcher -- the tools whose call ends the TURN, because the answer comes back as a later one (`consult_cogny` -- and since GH #530 that is the whole list: `ask_memory` was retired, not replaced). Declares async too -- the dispatcher unions the two lists, so one entry is enough and naming a tool in both is harmless, just redundant. `remember` did not belong here while it existed (GH #372) |

**`ctx.model` is the one instantiation-class knob** and it is strict: `add_nodes` without
it is rejected with `ctx_key_missing`. Two equally valid forms (session ruling
2026-08-15): pass a **resolved literal** (the K-H2 builder convention — the builder
resolves `MODEL_<ROLE>` from `.env` itself), or pass the **`${MODEL_<ROLE>}` token**
verbatim so the cell re-resolves it from `.env` at spawn — the examples use the token
form to stay vendor-neutral. Since `4.3.0` the composite carries exactly ONE `llm` cell,
the brain, so the key has one reader; a subscription or another provider for it is
`override_params` on `brain.params` (`provider`, `auth`, `auth_ref`, `base_url`).

**The TTL budget.** With `restore_ttl` on the seam the colony default of 64 carries the
loop: only ONE round has to fit the budget. A tree that removes the restoring edge sizes
`message_default_ttl >= 4 + rounds * 12` in its `colony.json` instead.

## Instantiating it

```bash
curl -s -X POST http://127.0.0.1:PORT/colony/mutations -H 'Content-Type: application/json' \
  -d '{"scope":"/main/agent","ctx":{"model":"openai/gpt-4o-mini"},"diff":{
        "add_nodes":[{"name":"talky","template":"talky"}],
        "add_edges":[ ... the four ports plus the tool lanes, in the SAME mutation ... ]}}'
```

The composite comes up with all twelve cells (plus three hive markers); the two `timer`s
spawn as soon as the crossing edge makes the subtree active, and the `store`/`llm` cells report
`active=true` + `NotYetSpawned`, which is the correct hot/cold form for a stateful cell.
Two things to have ready before the mutation:

1. **A `colony.db` whose three tables agree** (`registry`, `edges`, `hive_scopes` -- all
   empty or all filled). A mutation that is REJECTED leaves a colony whose next boot
   panics (GH #89), so bring the edges in the same diff and check the table counts after
   any rejection.
2. **The brain's identity**, either as `brain/seed/system.jsonl` (which only takes on a
   FRESH birth -- a `cell.db` that already exists means `Resumed` and an inert seed) or
   as a system update message. Neither is this template's business.

**The `identity` slot is a projection target.** The brain's `system_order` begins with
`identity` (`brain/config.json:14-18`), and that first slot is where a person -- the user,
the agent itself -- is rendered into the prompt. An `affinity` hive may push
into it: one edge per subscribing cell on `hop.route == 'answer' && hop.subscriber ==
'<this brain>'`, and every change to the record reaches the brain as a `system.*` write and
not as an inference (the recipe is in the `affinity` template's own README,
§ Wiring `out_push` for a subscribing brain). Nothing here configures it, and nothing here
needs to: the lane is the parent's business, exactly like the seed above. A brain with
nobody pushing into `identity` is not a broken brain -- `system_order` names the key it
would render first, and a `system` tree that does not carry the key is simply concatenated
without it (`crates/meclaw-cells/src/llm/translate.rs:56-60`); nothing declares it unbound.
Since
[#285](https://github.com/mmeyerlein/meclaw/issues/285) a hive port may be declared as a
slot (`{"name": "...", "slot": true, "unbound": "park"}`), so a composite that means to bind
the lane later says so in its contract from birth instead of parking a placeholder at the
address.

## What it is not

- **Not a surface.** No proxy, no HTTP ingress, no allowlist. Who is allowed to talk to
  the agent is an edge condition on the ingress port, in the parent scope where the
  surface lives.
- **Not a memory.** The recall leg is optional and the write batch leaves unfiltered.
  What a day is worth is the receiver's question.
- **Not a persona.** Identity and instructions live in the brain's `cell.db`, one writer
  per `system` path: the collector owns `messages[]` and `system.memory`, an affinity
  cell (if any) owns the rest. Since `4.3.0` (GH #447) no writer inside this composite
  owns `system.handover` at all, because there is no such slot any more. The topology
  owns none of that. **Tool schemas are the one place this line moved** (`4.2.0`, GH #55): the two
  tools the composite *implements* -- `memory_recall` and `thread_recall` -- ship with it,
  schema and edge together, and no others do. A tool the parent wires is still entirely
  the agent's, schema included.
- **Not a drain.** `./errors` normalises and forwards; it does not swallow. An unwired
  error port dead-letters, loudly.
- **Not one instance per day.** v1 runs the logical generation: same cells, new id.
- **Not the agent core.** The talky is the channel voice; the thinking and the heavy tool
  work belong to a `cogny` hive next to it (R-CG-1). The composite carries the two lanes to
  reach it and nothing of what happens there.
- **Not a memory, and neither is the core.** The long-term memory is not agent-level at
  all: a `memory-hive` is the source of truth of the **member**, and talky and cogny are
  two lenses on the same hive. Wiring a second agent for the same member does not mint a
  second memory.

## The credential connect point (GH #560)

Since `talky@4.6.0` the rim declares both halves of the **credential lane** and names
`./brain` as their connect point:

```json
"emits":  [{"route": "credential_request", "at": ["./brain"], "because": "…"}]
"accepts":[{"route": "in_sealed",          "at": ["./brain"], "because": "…"}]
```

`params.ports` stays literally `[]`. `at` is not a second kind of port: it is the
one opening this template pronounces about **itself**, for **one** named lane and
**one** address inside it (`docs/meclaw-overview.md` § *v-lanes*). What it buys is
that a member can wire this brain to the person's own `access` in **one** edge per
direction instead of a pass-through chain through three rims — and that neither
this level nor the generation above has to declare, forward or guard a lane it
takes no part in.

The brain accordingly ships `params.credential_grant_id` resolving to the empty
string, which is no grant at all (GH #271): standalone this composite behaves
exactly as it did before and spends its `api_key`. Switching it over takes **two**
`override_params` keys and not one, because a cell asks for a credential only
while it holds none — and `params.api_key` counts as one:

```json
"override_params": {
  "brain": {"api_key": "", "credential_grant_id": "grant:…"}
}
```

Set the grant and leave the shipped `api_key: "${OPENROUTER_API_KEY}"` standing
and the cell never asks: it keeps spending the environment key and the lane
carries nothing, silently, because a model that answers looks like a model that
answers. With both keys set the model runs with **no credential in its config** —
the value arrives sealed against an ephemeral key it mints per ask, is opened in
its own task and is written nowhere. Both keys are **immutable** (`docs/cell-types.md`
§ `llm`), so this is a birth act: a generation grown without the empty `api_key`
is repaired by growing another one, not by a message. The recipe, both edges and
the two operator gestures that go with them are in `templates/member/README.md`
§ *The credential v-lanes*; `examples/vault-pilot/` is the small runnable version
of the same round.

## Pins

- `crates/meclaw-cells/tests/talky_cogny_advisor.rs` -- the advisor connection end to
  end: an interim answer and a consult call out of ONE brain response, a round that
  closes without waiting, the agent core's own tool round, the result home on
  `in_advice`, and the bilateral question-back under one `consult_id`. Plus the pin that
  no idle window ever waits for the core (one-millisecond window, two sweeps, nothing
  swept).
- `crates/meclaw-cells/tests/talky_composite.rs` -- the shipped template in a running
  colony against the mock OpenAI wire: one turn through session-keeper, seam, brain,
  dispatcher, a tool and back to the seam (two provider calls, the second one carrying
  the tool result, the answer carrying the minted session id and `iter=1`); a close
  whose batch
  reaches the write port, which since `4.3.0` (GH #447) is the whole of the close path:
  nothing summarises it inside the hive and no further provider call happens.
- `crates/meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs` -- the
  two golden manifests over the instantiated tree (the sub-unit refs produce the same
  bytes the copies did) plus the stamp pin: a cell inside a referenced sub-unit carries
  its OWN template and names `talky` above it.
- `crates/meclaw-colony/tests/gh245_a_stub_names_a_lane_the_hive_admits.rs` -- the lane
  a curator stub names against the SHIPPED hive files: an edge stamping `in_thread_call`
  into the collector commits, an edge stamping `in_batch` is refused now that nothing
  behind the door reads it, and a real call on the lane crosses `talky`'s door and the
  collector's door and lands on the assembler.
- `crates/meclaw-cells/tests/w9a_per_turn_colony.rs` -- the per-turn lane in a colony
  that carries this composite and the memory hive's real write path, wired straight
  at the hive's writer port with nothing in between (GH #298 removed the `memory-drain`
  from this path): the turn AND the answer are `episodes` rows before anything closes,
  a second turn adds exactly its own two and re-writes neither of the first two, and
  the close that follows moves no row.
- `crates/meclaw-cells/tests/w10b_remember_colony.rs` -- the extraction lane in a
  colony that carries this composite, the shipped `memory-drain` and the memory hive's
  real write AND extraction path: one turn whose single response carries the answer and
  the annotation, the answer reaching the channel WITHOUT the fence, and the fact a
  candidate on the episode of the turn it answered, under the drain's own `turn_id`.
  Plus the other half, which is the one that makes inline extraction defensible at all:
  a block with a broken payload is not cut, writes nothing, covers no turn -- and the
  channel got its sentence anyway. Since GH #379 that second half also pins the one
  behaviour the flip changed: an unreadable block travels IN the answer rather than
  through `inline-reject`, because a parser that could not read the block does not get
  to edit the sentence around it.
- `crates/meclaw-cells/tests/gh379_the_splitter_cuts_the_sidecar.rs` -- the splitter's own
  three output forms, run through the shipped `params.script_inline` itself: a cut, a
  byte-identical pass-through (no block, and a tool-call round), and the flagged
  pass-through a block nobody can read earns. `talky_composite.rs`'s
  `an_annotated_answer_splits_into_the_reply_and_the_sidecar` is the same thing end to
  end -- the prose reaches the reply exit fence-free and the raw block leaves on
  `extraction`, for ONE provider call.
- `crates/meclaw-cells/tests/gh273_a_swept_close_reaches_the_memory.rs` -- a
  conversation ended the only way this template ever ends one, by a SWEEP, drained
  through the shipped `memory-drain` into the memory hive's real write path: the episode
  rows land with the room and the round of the CONVERSATION, although the sweep that
  ended it knows neither. The same property at the write port is pinned in
  `talky_composite.rs`.
- The sub-units keep their own pins: `session_keeper.rs`, `collector_window.rs`,
  `collector_colony.rs`, `dispatcher_template.rs`.
