# `talky@6.6.0`

A whole conversational agent as one template. Four referenced units under one hive:
[`session-keeper`](../session-keeper/), [`collector`](../collector/),
[`curator`](../curator/) and [`dispatcher`](../dispatcher/) -- each carrying its template's own name -- plus an
`llm` brain, the sidecar splitter, the declaration of this agent's own sidecar sections
and one error collector. No new cell type, no Rust.

**The first production rollout wired this by hand.** Keeper in the ingress, collector at the seam,
dispatcher for the fan-out, the close batch out to the write port -- fifty-seven edges,
each of them a decision that had already been made in a README. That is the definition of a
composite: a recurring unit that should be instantiated, not re-derived. Here it is one
`add_nodes` plus the four port edges the parent has to draw anyway.

## What it delivers

- **One id per conversation, minted once.** The keeper stamps every inbound turn with
  the generation its channel is currently in; the internal edge promotes
  `hop.session_id` to context, and that promotion *is* the stamp. Everything downstream
  reads it and nobody else mints one.
- **The seam, already bounded.** Since `6.0.0` the curator hands the window to the brain
  over ONE edge (GH #889), and that edge carries the two things the loop needs: the iteration
  counter and `restore_ttl`. A tool round is a dozen routing hops; without the restoring
  edge the fifth round dies mid fan-in with nothing emitted towards the surface.
- **A tool round that only needs its tools.** `brain -> splitter -> dispatcher -> (your
  tools) -> collector -> curator -> brain` is pre-wired except for the one lane that is genuinely
  per-instance: which cell answers to `web_search`. Adding a tool is one edge pair, never
  a topology change.
- **A close that hands the day on, once.** When a generation ends, the curator's batch
  (the collector's until `6.0.0`, GH #889) leaves on the write port as it is: the parent decides where a day
  belongs, and the member above the talky turns the close batch into the memory hive's
  close pass (GH #447). Inside this hive a close costs one model call, and it is off the
  hot path: the curator condenses the closed session into a handover note through its own
  summarizer (GH #896), and the next generation reads it on its first call as the leaf
  `history.handover`. Whatever else it needs of the last one comes back with the recall
  bundle of a turn; no `system.handover` slot is written behind the composite's back.
- **One place errors leave from.** The brain's failed inference, its content filter and
  the session-keeper's store refusal -- three failure lanes from two cells -- fan into
  `./errors` and leave as one normalised report. A parent drains one edge, not three
  lanes.

## Cells

| path | type | from |
|---|---|---|
| `session-keeper/{stamp,close,sessions,night,porter}` | `code`, `code`, `store`, `timer`, `code` | `session-keeper` **(sealed)** |
| `collector/{assemble,window}` | `code`, `store` | `collector` **(sealed)** |
| `curator/{intake,policy,writer,ledger,summarizer,clock,schemas,history,push,handover}` | `code`, `code`, `code`, `store`, `llm`, `timer`, `code`, `code`, `code`, `code` | `curator` **(sealed)**, since `6.0.0`; `schemas`, `history`, `push` and `handover` since `6.1.0` (GH #892, GH #893, GH #895, GH #896) |
| `dispatcher` | `code` | `dispatcher` (a single-cell template) |
| `brain` | `llm` | this template |
| `schemas` | `code` | this template |
| `splitter` | `code` | this template |
| `errors` | `code` | this template |

**The braces are an inventory, not an address list.** The three sealed sub-units declare
`params.ports: []`, so `./session-keeper`, `./collector` and `./curator` are the only sub-unit
addresses an edge from outside may name; `./session-keeper/stamp` and
`./collector/assemble` are refused with `hive_port_boundary`. Which cell inside picks the
message up is decided by the `in_` lane the edge sets, by the hive's own door edges. That
is what lets the inside of a sub-unit change without touching a caller.

### How the sub-units are referenced: by name and version (GH #277)

The four sub-units are **references**, not copies. Each of the four directories holds
one `config.json` and nothing else:

```json
{"cell": {"type": "ref", "template": "collector@5.1.0"}}
```

At instantiation the referenced template's tree takes that position, so the instance is
byte-for-byte the tree the copies used to produce -- and every cell inside it now records
the template it really came from: `collector/assemble` is stamped with the `collector` version it was grown from, with
`talky@6.6.0` above it in its provenance chain. `5.2.2` moves the `collector` pin to
`4.2.1` ([#728](https://github.com/mmeyerlein/meclaw/issues/728)): the answer of an advice or a
delegation round carries the member's turn, and `hop.late` beside it. The same version gives
`brain` the OpenRouter app attribution (`http_referer` / `x_title`, overridable by
`OPENROUTER_HTTP_REFERER` / `OPENROUTER_X_TITLE`, the form the memory-hive cells use), so a
talky's request names its app at the provider. `5.3.0` moves the `collector` pin to `4.3.0`
([#834](https://github.com/mmeyerlein/meclaw/issues/834)) and carries its brief leg across
this rim: `brief` leaves `./collector` for `.`, and `in_briefing` joins the entrance list into
`./collector` beside `in_bundle`. `5.4.0` moves the `collector` pin to `4.4.0` and the
`dispatcher` pin to `1.2.1` and sends a completion cut on `length` through the splitter
([#843](https://github.com/mmeyerlein/meclaw/issues/843)): `./brain -> ./splitter` takes
`length` beside `stop` and `tool_calls`, and `./splitter -> ./collector` restamps it to
`in_answer` -- the old `./brain -> ./collector` edge sent it past the splitter, sidecar and
all, into an answer that looked complete. `length` never reaches the dispatcher, and the
answer that leaves carries `hop.finish_reason` and `hop.truncated = "1"`. The knob `brief_slots` stays empty here -- a standalone talky
has no record of people beside it, and a brief that leaves for an address nobody wired would
hold every turn with a counterpart for ever; the assistant's ref markers set it, where the
member's brief road is drawn. `5.4.3` moves the `collector` pin to `4.4.1` and the
`dispatcher` pin to `1.2.2` ([#871](https://github.com/mmeyerlein/meclaw/issues/871)): the
splitter hands the block it cut out of an answer on as the body slot `sidecar_raw`, the
dispatcher passes it through, and the collector shows every earlier answer with its block in
the window (*The block rides beside the answer*, below). The same version shows a sentence
said beside a tool call with the memory contract's nothing form (knob `nothing_block`), and
reads an unfenced `{"memory": {...}}` as one block (*The legacy fence still reads*, below).
No lane and no edge moved, so it is the third digit.
`5.4.4` declares `hop.model` on `brain` ([#886](https://github.com/mmeyerlein/meclaw/issues/886)): the `llm` cell has always
written the model the provider served into that header slot, and the contract now says so, in the
wording every other `llm` cell of the library uses. No lane and no edge moved, so it is the third digit.
`6.0.0` references `curator` and moves the window, the episodes, the close batch and the
identity pack from the collector to it ([#889](https://github.com/mmeyerlein/meclaw/issues/889)); `in_prune`,
`in_thread_call` and `prune` left the boundary with them, so it is the first digit. The same
number declares on `brain` every hop key the `llm` cell writes into an answer, the cache keys
among them ([#890](https://github.com/mmeyerlein/meclaw/issues/890)).
`6.1.0` moves the `curator` pin to `1.1.0` and the `collector` pin to `5.1.0` ([#892](https://github.com/mmeyerlein/meclaw/issues/892),
[#893](https://github.com/mmeyerlein/meclaw/issues/893), [#894](https://github.com/mmeyerlein/meclaw/issues/894), [#895](https://github.com/mmeyerlein/meclaw/issues/895), [#896](https://github.com/mmeyerlein/meclaw/issues/896)): the curator runs as role `talky` and takes the `window`, `gap`
and `memory` sections from the splitter, answers the `history_*` tools and builds the memory
leg's question inside, the splitter takes the short block ids out of everything that leaves,
and the new lane `in_renewed` carries a renewed duplex call to the curator. A lane joined the
boundary, so it is the second digit.
`6.1.1` moves the `curator` pin to `1.1.1` ([#904](https://github.com/mmeyerlein/meclaw/issues/904)): the curator's cache clock keeps one
standing order instead of adding one per call. Only the pin moved, so it is the third digit.

**The library has to carry the four.** A reference resolves against the colony's template
registry, so `collector`, `curator`, `session-keeper` and `dispatcher` have to sit in
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
| `in_turn` | in | the surface turn. The edge MUST promote the channel identity to `context.channel`, and the round to `context.audience_set` if closed sessions are to reach a memory. Since 5.4.0 the rim carries two more things through to the collector untouched: the channel's tool scope (`context.tools_allow` / `context.tools_deny`, stamped by the channel edge, constant per session -- the collector sends it with every brain call of the session as `tool_scope`, through `./curator` since 6.0.0, [#845](https://github.com/mmeyerlein/meclaw/issues/845)) and turns of `origin: "peer"` with their `speaker` / `speaker_ref`, which the collector keeps as role `peer` and never as this agent's own answer ([#847](https://github.com/mmeyerlein/meclaw/issues/847)). See the collector's README, "The channel's tool scope" and "The other side's words" |
| `answer` | out | the finished turn. **Three** sorts since `collector@2.1.1`: a real answer, a round that hit `max_iter` (`hop.round_capped`, and since `collector@3.5.0` `hop.partial == "1"` with a named partial answer as its last turn, #570), and a turn the store refused to let be assembled (`hop.degraded`, which carries no `round_capped`) |
| `write` | out | the closed session as one batch |
| `error` | out | a normalised failure report. **MUST** be wired |

The rest, each optional and each still at the same address:

| lane | direction | what travels |
|---|---|---|
| `tool` | out | a tool call for a cell you wired; `hop.tool_name` says which |
| `turn_write` | out | **one message per turn, never a batch** (GH #298): one `user`/`assistant` turn, `hop.turn_id` = `<session_id>#<index>`, `hop.turn_index` and `hop.happened_at`. On unless the instance switches it off at `curator/writer` (since 6.0.0, GH #889) -- see "Per-turn episodes" |
| `recall` | out | a memory read this turn needs |
| `in_tool` | in | one tool result coming back |
| `in_advice` | in | an advisor's answer coming back |
| `in_delegation` | in | an errand the voice model handed the backend of its own accord, in a duplex call. It comes STRAIGHT from the channel, not through a firewall -- a firewall's exit stamps `in_turn`, and this is no turn of the conversation. Promote `context.delegation_id` (the correlation the answer travels back under) and `context.engine`. Since 5.2.0 |
| `in_renewed` | in | a duplex voice session of a running call was renewed at its provider's limit (GH #896): `hop.call_id`, `hop.renewal_n`. It goes to `./curator` only -- no round, no model -- and comes back out as ONE `sidecar` section `context` carrying `hop.renewal_n`, with `context.call_id` set on the way in, so the member's advice edge hands the running conversation to the call's new session |
| `in_bundle` | in | a memory bundle coming back |
| `in_sweep` | in | an operator-forced session sweep |
| `in_round_sweep` | in | the operator lane of the collector's round table: a round that ran out of iterations |
| `in_pack` | in | a durable `system.*` slot for the brain: `identity`, `identity_short`, `persona`, `handover` or `instructions`, and nothing else. **Paired**: see `pack_ack`. Since 4.4.0 |
| `in_pin` | in | a pin of another hive for the brain's window, `{pins: [{text, source, until?}], replace_sources?}`, handed to `./curator`'s own `in_pin` (`templates/curator/README.md`). Nothing answers it. Since GH #916 |
| `pack_ack` | out | the receipt `in_pack` answers with, accepted and refused alike: `hop.pack_owner`, `hop.pack_slots`, `hop.error_code` (empty, `slot_unknown` or `pack_empty`), `hop.pack_unknown`. Since 4.4.0 |
| `in_model` | in | a model package for the brain: a **params-only** body (an empty `system` slot, no `messages`) the colony's `llm-registry` pushes. It goes straight to `./brain`, past the collector, and nothing answers it; since 6.0.0 a package whose `hop.subscriber` ends on `/curator/summarizer` goes to `./curator` instead (GH #889). See "The model door". Since 5.4.0 ([#855](https://github.com/mmeyerlein/meclaw/issues/855)) |
| `model_refused` | out | a model push the brain refused: its error, with `hop.refused_subscriber` (the brain's path) and `hop.refused_model`, instead of on `error`. Draw it back to the registry beside the push edge, or it dead-letters `no_route`. See "The model door". Since 5.4.2 ([#863](https://github.com/mmeyerlein/meclaw/issues/863)) |
| `schemas` | out | the tool names this agent declares it uses (`{"tools": [...]}`), for a tools hive's `in_schemas` door. It leaves on a TICK, not per turn. **Paired**: see `in_menu`. Since 4.5.0 |
| `in_menu` | in | their declarations coming back, plus the names that hive had nothing under. Since 4.5.0 |
| `in_export` | in | a demand for the session ledger as a versioned document. It crosses to `./session-keeper` unchanged and the keeper's own walk answers it. **Paired**: see `dump`. Since 4.5.0 |
| `in_import` | in | one part of such a document, for a keeper that is already running. Same crossing, same hive, same pairing. Since 4.5.0 |
| `export_done` | out | the keeper inside this composite wrote its whole session ledger itself and says where: `hop.seed_dir` (relative to the fence its store declares), `hop.export_hive`, `hop.export_of`, `hop.rows_written`. Carried out unchanged -- this level owns no part of the document. Since 5.0.0 ([#555](https://github.com/mmeyerlein/meclaw/issues/555)) |
| `dump` | out | the receipt of one applied import part (`hop.rows_written`, `hop.export_final == "1"` on the last). Since #555 that is all this lane carries. **Drain it with a PLAIN `hop.route == 'dump'` test** -- an edge that also tested a second hop key reads as no drain under the `required_drains` probe. Since 4.5.0 |

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
same lanes through and [`member`](../member/README.md) carries them out; since #555 the
keeper's own store writes the ledger and nothing on the way files anything. A member's
whole recipe is in [`../member/README.md`](../member/README.md) § *The export, and the cell
this level no longer owns*.

**`in_prune` and `prune` left with `6.0.0`**: the window they cut moved to `curator`,
whose ledger only appends ([#889](https://github.com/mmeyerlein/meclaw/issues/889)).

**The lane names are the contract, the cell names are not.** `session-keeper`,
`collector`, `curator`, `dispatcher` and `errors` are implementation: they may be renamed, split
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

### The tools the composite serves itself

**The model reads its own wall since `talky@6.1.0`**
([#893](https://github.com/mmeyerlein/meclaw/issues/893)): `history_search`, `history_read`
and `history_outline`. The wall is `curator`'s ledger, a store no cell outside that hive may
read, so the three are served inside it and never leave on the tool lane. One ordinary edge
takes every name that starts `history_` to the curator, one takes its answer to the collector:

```json
{"from": "./dispatcher", "to": "./curator",
 "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name.startsWith('history_')",
 "modifier": {"set_hop": {"route": "'in_history_call'"}}}
{"from": "./curator", "to": "./collector",
 "condition": "!has(hop.refused_subscriber) && has(hop.route) && hop.route == 'tool_result'",
 "modifier": {"set_hop": {"route": "'in_tool'"}}}
```

The curator also declares the three, as a second answerer of the collector's menu question,
and `./collector`'s `params.tools` names them; a channel's `allow` list stays its whitelist,
so a channel that narrows the tools and wants these names them too. They take the place of
`thread_recall`, the collector's round-table lookup, retired with `talky@6.0.0`
([#889](https://github.com/mmeyerlein/meclaw/issues/889)). Every other name leaves on the exit
below -- `memory_recall` since `5.0.0` ([#552](https://github.com/mmeyerlein/meclaw/issues/552)).

**The tool exit stopped naming names.** `./dispatcher -> .` on `hop.route == 'tool'` is a
**guarded default edge** since `4.2.0` ([#283](https://github.com/mmeyerlein/meclaw/issues/283),
ruling Q1): it is consulted only when no ordinary edge out of `./dispatcher` fired for the
message. An ordinary edge on a reserved name silences the exit for itself
and no term of exclusion is written anywhere. A reserved name would cost one ordinary
edge and **no** change to the exit at all -- which is the whole difference between a default
edge and the negation chain it replaces. It is also what made `memory_recall` and
`thread_recall` cheap to give back: one edge deleted each, and the name leaves on the default
like every other tool.

**Two properties keep that honest, and both are measured against this tree rather than
assumed.** The guard is not decoration: `./dispatcher` emits four sorts (`calls`, `result`,
`answer`, `tool`) and default suppression is **sender-wide**, so an unguarded default would
try to carry `calls`/`result`/`answer` outward whenever nothing ordinary happened to fire
for them. And there is **no unconditional tee**: `./dispatcher` has five out-edges here and
every ordinary one is conditioned on its own lane. If you add a tee of your own -- a logger,
a tap, a mirror, at `./talky/dispatcher` -- condition it on its own routes, or it silences
this default for every tool call and your tool cells go dark.

**The brain's seed went with it:** `brain/seed/system.jsonl` carried the `thread_recall`
schema and nothing else, and since `6.0.0` this composite ships no system state at all
([#889](https://github.com/mmeyerlein/meclaw/issues/889)).

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

Seven more lanes, all of them the parent's decision since `6.0.0` -- the thread tool the
composite drew itself left with GH #889. Every one of them lives
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
| memory tool | **the parent's** -- since `5.0.0` `memory_recall` leaves on the ordinary tool exit and the parent routes it to a memory hive, which declares the schema and answers the call | GH #552. Up to `4.6.1` the composite answered the name itself and this row said "nothing to wire"; it does not any more. The `assistant` level ships the four edges (`templates/assistant/README.md`), and the recall pair above is the same road's other leg |
| forced sweep | `./talky` lane `in_sweep` | an operator or a second schedule |
| housekeeping | `./talky` lane `in_round_sweep` | a timer; the template never fires it itself. `in_prune` left with `6.0.0` (GH #889) |
| per-turn write | `./talky` route `turn_write` out | one message per turn into a memory hive's `in_episode` lane; on by default -- see below |
| memory lookup | **nothing to wire any more -- RETRACTED in `4.5.1`** (GH #530). The `ask_memory` errand is retired; a fast memory question is the `memory_recall` row two lines up, answered since `5.0.0` by the member's own memory hive | the row used to send that question to a core that has no memory leg. See *The one errand, and the memory question that is not one* |
| the sidecar | `./talky` on `hop.route == 'sidecar'`, distributed on `hop.section`: `section == 'memory'` to the memory hive's `in_remember` lane, **plus** its `reject` egress into the parent's own drain | one message per section of the block a turn's answer carried -- see "The sidecar". Since `talky@5.1.0` the port is `sidecar` and not `extraction`, and the memory annotation is one section on it (GH #605); before that the lane carried nothing else. Since `talky@4.1.0` it is a ROUTE and not a tool name: a parent still wired on `hop.tool_name == 'remember'` writes nothing |

### Per-turn episodes (`turn_write`)

The write port fires at the **close**. For a day archive that is right; for a memory it
means nothing said today is retrievable until the night sweep has run, so a question
about the last exchange is answered out of an empty store. The `turn_write` lane closes
that hole, and since GH #298 (ruling Q11) it is the **only** path from this conversation
into an episodes table -- which is why it is on by default.
Since `6.0.0` it leaves `./curator`'s writer, in the contract `collector@4.4.1` shipped
([#889](https://github.com/mmeyerlein/meclaw/issues/889)).

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
is the deterministic id `./curator`'s writer mints, and an edge that promotes the context key writes
episodes the inline bind can never find.

**The `write` route is not a second half of this.** It carries a closed session with its
`rounds` slot, for whoever archives a day; wiring it into the same memory would be a
second writer over turns this lane already wrote. The two documents are different on
purpose (GH #298) -- and whoever consumes `write` for something else (a day archive, the
memory hive's close pass) is untouched, because `turn_write` is a route of its own
precisely so that the close-only consumers stay close-only.

**Delivered twice is written once** -- moved to `curator` with the writer (GH #889),
and the `turn_id` is deterministic, so a repeat is recognisable downstream as well.

## The internal wiring, edge by edge

Twenty-two edges of round in this hive's `params.graph` -- plus the thirty-five that ARE the
boundary (sixteen door edges from `.`, nineteen leaving towards it, and those are the lanes
above; the thirteenth is the brief leg's request, GH #834, the fourteenth a refused model
push, GH #863, and ten of them leave `./curator` -- `write`, `turn_write`, `pack_ack` and
its summarizer's `model_refused` since 6.0.0, GH #889, `sidecar` for the `memory` section it
passes on unchanged, GH #892, and for a gap's find spoken as `fact` in a duplex call, the
memory ask, the collector's and a gap's own, GH #895, and the answer to a stats question,
`stats`, GH #926, the things its `things` section saw and a candidate's receipt, `thing_seen`
and `candidate_ack`, GH #949, and the answer to an app's ledger read, `read`, GH #949; the sixth door is the mutation
receipt, GH #553, the seventh is the `in_menu` fan that reaches `./schemas` beside the
collector, GH #783, the eighth is the model door straight into `./brain`, GH #855, the ninth
the pack door and the tenth the summarizer's model door, both into `./curator`, GH #889, the
eleventh a gap's bundle into `./curator`, GH #895, the twelfth the renewed duplex call,
`in_renewed` into `./curator`, GH #896, the thirteenth door another hive's pin,
`in_pin` into `./curator`, GH #916, the fourteenth door an observer's stats question,
`in_stats` into `./curator`, GH #926, the fifteenth a push candidate, `in_candidate` into
`./curator`, GH #949, and the sixteenth an app's ledger read, `in_read` into `./curator`, GH #949).
The two halves are the whole of this file, counted from it. Every one of the twenty-two names a
sub-unit **by its path**: three of the eight nodes below are sealed hives, so the address is
the hive and the lane in the third column is what the door behind it reads; what those three
draw INSIDE themselves is theirs and is not counted here. Read it as the round it is:

```
session-keeper --(turn, session_id -> context)-->  collector   in_turn
session-keeper --(close, session_id + channel + audience_set -> context)->  curator   in_close

collector ==(curate, int(hop.iter) < 12, restore_ttl)==> curator  in_curate  <- the whole round, GH #889/#919
curator ==(brain, int(hop.iter) < 12, restore_ttl)==>  brain      <- THE SEAM
collector --(menu)--------------> curator    in_slots   <- the answered tool menu, GH #464
collector --(schemas)-----------> curator    in_schemas <- the curator answers the menu too, GH #892
curator --(tool_schemas, !refused_subscriber)--> collector  in_menu  <- tool_answerer 'curator', GH #892
collector --(recall)------------> curator    in_recall_ask  <- its question is built there, GH #895
brain --(any answer, !refused_subscriber)--> curator  in_llm   <- the tap, GH #889
   .      --(in_pack)-----------> curator    <- THE DOOR IN THE WALL, GH #458
   .      --(in_pin)------------> curator    <- another hive's pin, GH #916
   .      --(in_stats)----------> curator    <- an observer's question, GH #926
   .      --(in_model)----------> brain      <- THE MODEL DOOR, past the collector, GH #855
   .      --(in_model, subscriber ends /curator/summarizer)--> curator   <- GH #889
   .      --(in_bundle, context.gap_ask)--> curator  in_gap_bundle  <- a gap's find, never the collector's, GH #895
schemas --(operation == schemas)-> collector  in_menu   <- this agent's own sidecar offer, GH #783
brain --(stop | tool_calls | length)--> splitter  <- the sidecar cut, GH #379; length since 5.4.0, GH #843
splitter --(stop | tool_calls)---> dispatcher
splitter --(length)--------------> collector    in_answer   <- a cut answer, its sidecar cut too
splitter --(sidecar: window | gap | memory)--> curator  in_section  <- the curator's sections, GH #892
splitter --(sidecar, any other)-->  .        <- one per section, out of the sidecar port
brain --(error | content_filter, !refused_subscriber)-> errors
brain --(has(refused_subscriber))--> .   route := 'model_refused'  <- a refused push, GH #863
session-keeper --(reject)--------> errors    <- the session store refused a step
curator --(turn_write, !refused_subscriber)--> session-keeper  in_answered  <- the answer receipt: a sealed generation closes after its last answer stands on the wall, GH #953

dispatcher --(calls)---> collector   in_calls    dispatcher ==(tool, DEFAULT)==> [your tools]
dispatcher --(result)--> collector   in_tool
dispatcher --(answer)--> collector   in_answer
dispatcher --(tool, history_*)--> curator    in_history_call   <- the model's own wall, GH #893
curator --(tool_result)----------> collector  in_tool

curator --(write)------------>  .            <- the close batch, out of the write port
curator --(pack_ack)--------->  .            <- the pack receipt, GH #458
curator --(model_refused)---->  .            <- the summarizer's refused push, GH #889
curator --(stats)------------>  .            <- the answer to it, GH #926
curator --(sidecar)---------->  .            <- the memory section, unchanged, GH #892; a gap's find as `fact` in a duplex call, GH #895
curator --(recall)----------->  .            <- the memory ask; a gap's own lifts gap_ask into context, GH #895
collector --(schemas)--------->  .            <- what tools this agent declares, GH #464
collector --(brief)----------->  .            <- the counterpart's brief, GH #834

[sealed]  session-keeper  collector  curator   [plain]  brain  schemas  splitter  dispatcher  errors
```

**The edges inside the sealed sub-units are not drawn here, and that is the point.** A sealed sub-unit takes its
lane at its own `{"from": "."}` door edges and distributes behind them -- `session-keeper`
alone brings eighteen edges, `collector` five, `curator` its own
([`../curator/README.md`](../curator/README.md)) -- and none of that is
visible to, or wireable by, the hive above. What the edges above state is the
whole of talky's own topology.

**The one `==` in the fan-out block is the default edge.** `dispatcher --(tool)--> [your
tools]` is consulted only when no ordinary edge out of `./dispatcher` fired for the message;
since `6.0.0` no ordinary edge claims a tool name (GH #889).

**The loopback bound is an edge literal, on purpose.** `int(hop.iter) < 12` is a safety
belt, not the policy: the round is bounded by `max_iter` (default 8), which
ends a runaway round with a message on the `answer` lane instead of a silence. The edge
number only has to be larger. Env substitution does not reach edge conditions -- a
`${VAR}` there would be registered verbatim and fail to parse as CEL -- so raising it is
a mutation: `remove_edges` first, `add_edges` second, in **two** mutations. A remove and
an add of the same endpoints in ONE diff match over the post-state and take the new edge
with them.

**`restore_ttl` sits on two edges of the seam, each once per round.** `iter` counts brain
answers, and a bundle of fifteen calls is one answer, one iteration: the edge into the brain
restores once and the `curate` edge restores once, both under the same bound. The substrate refuses
a restoring edge without a condition, because the iteration bound is then the only thing
left stopping the loop.

**The seam starts at the collector (GH #919).** Since #889 the curator stands between the
collector and the brain, and its intake, policy and handover spend about twenty routing
decisions of ledger round trips on every round. The `curate` edge restores the budget too,
under the same bound: otherwise the curator pays out of whatever the legs before it left.
Measured on the shipped road: the memory's recall leg costs 26 decisions, a turn with an
audience reached the curator with 20 left and crossed into the brain at 1, and one level
more above the generation dead-lettered it `ttl_expired` inside the curator.

### The door in the wall (`in_pack`, GH #458)

**Moved to `curator`** ([#889](https://github.com/mmeyerlein/meclaw/issues/889)): since
`6.0.0` the pack enters `./curator`, which holds what the closed list allows -- `identity`,
`identity_short`, `persona`, `handover`, `instructions` -- in its ledger and hands it to the brain with the next
call; the lane, its receipt and the edge that opens it are unchanged at this rim.

**`in_pack` and `pack_ack` are one decision**, paired in `params.required_drains`. The
receipt answers unconditionally, because from the sending side a push that landed and a
push that reached nothing are the same silence otherwise.

```json
{"from": "<member>/affinity", "to": "./talky",
 "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber == './talky'",
 "modifier": {"set_hop": {"route": "'in_pack'"}}},
{"from": "./talky", "to": "<log or drain>",
 "condition": "has(hop.route) && hop.route == 'pack_ack'"}
```

This pair delivers but books nothing: affinity counts a pack as delivered only when the door
edge also promotes `pack_sub` and `pack_hash` into context and the receipt comes back into
affinity's `in_pack_ack` (GH #877) -- the builder's corridor draws both (`templates/affinity/README.md`
§ Wiring `out_push`); a receipt into a log or drain leaves affinity sending the pack again on its
retry schedule.

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

The composite ships no persona and no instructions -- since `6.0.0` no brain seed at all
(GH #889). So the brief that makes an agent subscribe belongs to the **instance**, verbatim,
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

### The model door (`in_model`, GH #855)

**A model package reaches the brain through `in_model` and nothing else.** The seal that
keeps `./brain` out of reach (`hive_port_boundary`) keeps the colony's own model registry out
too, and until 5.4.0 that was complete: `llm-registry` could resolve a package for this
brain and had no edge to send it along. `in_model` is the one door for it, and it is drawn
from `.` straight to `./brain`:

```json
{"from": ".", "to": "./brain", "condition": "has(hop.route) && hop.route == 'in_model'"}
```

Since `6.0.0` a second edge on the same lane reaches `./curator` when `hop.subscriber` ends on
`/curator/summarizer` (GH #889).

**Past the collector, on purpose.** What travels here is a params-only body --
`{"system": {}, "params": {"model": ..., "model_prompt": ..., "$reset": [...]}}`, an empty
`system` slot and no `messages` -- which the `llm` cell merges into its live params, persists in its own `cell.db`
and answers with nothing (`docs/cell-types.md` § `llm`). It is not a turn: the collector
would have nothing to assemble, nothing to stamp and nothing to wait for, and a lane
through it would be a round with no answer. So the door skips it, the brain stays silent,
and there is no receipt and no drain to pair.

**A conversation cannot change the model it talks to.** The brain takes package keys from a
params-only message only; a `params` slot riding beside turns is ignored with a stderr line
(GH #853). So the door is the whole of the way in, and the edge onto it is drawn once, by
whoever grows the generation -- in `meclaw-os` the builder's `grow_level assistant`, from the
registry's `update` lane, restamped `in_model` and addressed by `hop.subscriber` (the brain's
cell path). `templates/llm-registry/README.md` has the precedence, the package and the ops.

**The brain says what it needs, in prose** (since 5.4.1, [#858](https://github.com/mmeyerlein/meclaw/issues/858)).
`./brain` carries `params.requirement`: a voice that answers a person turn by turn and must start
within seconds, reads a context of some thousands to some tens of thousands of tokens, calls
tools reliably, hands the deep work to a reasoning core and runs on every turn, so the price per
call stays low -- and no model name. A registry translates it against its catalogue once per
change and keeps the result; the builder's `grow_level assistant` announces it with the brain.
The param is immutable and inert without a registry: the brain runs its start value.

**A refused push goes back** (since 5.4.2, [#863](https://github.com/mmeyerlein/meclaw/issues/863)). A push the brain refuses -- a `base_url` outside its
`base_url_allow`, a timeout its backstop does not clear -- is no conversation's error, and before 5.4.2 it reached `./errors` as one. The refusal of a push
addressed to the cell itself carries two header keys of its own, `hop.refused_subscriber` (the
cell's path) and `hop.refused_model` (the model the push named); every other out-edge of the
cell excludes it, and one way back per cell leaves the hive with it as `model_refused`:

```json
{"from": "./brain", "to": ".", "condition": "has(hop.refused_subscriber)",
 "modifier": {"set_hop": {"route": "'model_refused'"}, "delete_context": ["tool_answerer"]}}
```

In `meclaw-os` the builder draws the way back beside the push edge (`grow_level`, one edge from
this composite to the container), and the shell carries it on to its registry's `in_refused`
lane, where `show` names the refusal until the next push or a `reset`
(`templates/llm-registry/README.md` § A refused push). Without that edge the refusal dead-letters
`no_route`, loudly. A refusal of anything else -- an operator's push without an address, a push
addressed to another cell -- keeps the shape it had and the edge it always took: the forward that
caused the second is the defect.

### The menu is asked for, not typed (`schemas` / `in_menu`, GH #464)

Until 4.5.0 a talky's tool declarations were a list somebody had written into its brain's
`system.tools`, or into the collector's `tool_menu`. Adding a tool to a colony meant editing
that list in every agent that might use it, and no agent could offer a model anything nobody
had typed.

**What the composite owns is the DECLARATION.** `./collector`'s `params.tools` names the
tools this agent uses -- shipped as `["web_search", "web_fetch"]` plus the three `history_*`
names its own curator answers, `["*"]` for everything a tools hive has -- and the schemas
behind those names are asked for:

```json
{"add_nodes": [{"name": "scribe", "template": "talky@6.6.0",
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
hands the result on `menu` to `./curator`, which writes it into `./brain` as `system.tools`
with the next call (since 6.0.0, GH #889). An `llm` cell
upserts `system.*` per slot path into its own `cell.db`, so the menu costs **one write per
change and nothing per turn**. It is the same door `in_pack` uses, and it
is durable for the same reason.

**The ask is not a birth, and since `5.0.0` it is not a tick either.** The substrate hands a
cell no message at spawn, so nothing can ask at boot by itself: what asks is the MUTATION
RECEIPT. This composite declares `mutation_committed` at its own rim -- the lane keeps ONE
name from the mutation door down to the collector, which is what lets every level in between
declare it and be a mandatory hop for it -- and carries it inwards with one edge of its own,
`. -> ./collector`. The boot receipt is the first one,
so an agent has its menu before its first turn, and a tool ADDED to the hive later reaches this
agent with the receipt of the very mutation that added it. Until `5.0.0` a `timer` inside the
collector (`./collector/menu-clock`, `MENU_CRON`) did it every five minutes
([#553](https://github.com/mmeyerlein/meclaw/issues/553)).

**A name nobody has is named.** It comes back in `hop.menu_unknown` and as a warn line in
`log.jsonl`; a declaration pointing at nothing is a defect in this agent's own template, and
the value of declaring is that it is visible. The typed `tool_menu` override left the
collector with `6.0.0` (GH #889).

**Since `3.4.0` the collector MERGES the answers of several answerers**
([#529](https://github.com/mmeyerlein/meclaw/issues/529)). One tick asks every answerer
the tree wires at once, and each reply says who sent it in `context.tool_answerer` -- the
mirror of the `context.tool_caller` the request already carries. The collector keeps each
answerer's last submenu as one row of its own store, keyed by that name, and on every reply
writes the UNION of the rows, with `$replace`. Before the
merge one answer *was* the menu, which is right while exactly one thing answers and is the
whole defect the moment two do: the second reply would not join the first, it would delete
it, and the two would take the menu away from each other on every tick forever. **A reply
with no `tool_answerer` counts as the default answerer**, so a tree wired the old way
behaves exactly as it did.

`hop.menu_unknown` moved with it: it is computed against the MERGED menu, so a name one
answerer has nothing under is not a finding when another answerer delivers it.

**Outside itself this composite still asks exactly one answerer, and that is deliberate.** Standalone, a
talky has a tools hive beside it and nothing else that could serve a tool name, so its
shipped declaration is its two search tools -- plus the three `history_*` names its own
curator answers inside, where no parent can reach (GH #893). The declared list grows **one level up**: in
[`../assistant/README.md`](../assistant/README.md) the level adds `consult_cogny` to it and
draws the `schemas` / `in_menu` pair a second time, out to the reasoning core -- because
`consult_cogny` is not a tool of any hive, it is an errand that LEVEL routes, so only the
side that answers it can declare it. That is the level's decision for the same reason the
model is (GH #516), and neither the list nor the second pair belongs in this template.

The full account of the mechanism lives in
[`templates/collector/README.md`](../collector/README.md) § *The menu is asked for*, and the
answering side in [`templates/tools/README.md`](../tools/README.md) § *Asking for the
declarations*.

### The sections this agent offers its own model (`./schemas`, GH #783)

A tool declaration and a sidecar section are two answers to the same question -- what does
this agent get to ask its model for -- and since GH #606 the collector merges both halves
off the same lane. So the sections this composite asks for are declared the way a tool is:
by a cell, beside the agent that consumes them. `./schemas` is that cell, and it declares
**no tool at all** -- the tools of a talky are the parent's, and what this cell has to say
is the other half of the question.

**Three sections, all optional**, and they exist for the occasion on which this agent is
not the one speaking. On a duplex voice call a voice model talks to the caller on its own
timeline and this agent ADVISES it, so what leaves the brain is not a reply:

| section | what it is | how it is consumed |
|---|---|---|
| `fact` | what the caller should HEAR next, as one or two plain sentences | the voice model paraphrases it aloud |
| `context` | what the voice model should KNOW silently -- recall, profile, application state | never spoken on its own |
| `correction` | a rule for the REST of the call: a guardrail, a redirect | it changes the model's behaviour until the call ends |

**Outside that mode the three stay empty, and the offers say so themselves.** Every one of
the instructions carries `in advise mode only`; the mode itself is a slot of
`system.instructions` and the model reads both. A section is offered ONCE, at composition
time, and the same block contract stands in the brain for every turn the agent takes --
switching one on and off per turn would need a second mechanism nothing in the block could
read, and a section marked required while it is empty on most turns is a contract a model
learns to break. `correction` is the one to spend sparingly: nothing on the model's
timeline expires one.

**It rides the `in_menu` fan, and it answers as an answerer of its own.** The composite's
door edge for that lane reaches `./schemas` beside `./collector`, and the edge back stamps
`context.tool_answerer` with `talky`: without a name of its own this cell and the tools
hive would delete each other's row from the collector's menu table on every tick, which is
the defect GH #529 was built for. It follows that the offer travels on an ANSWER -- a tree
that wires no menu answerer at all asks nothing, hears nothing, and its brain carries the
block contract it carried before.

### The advisor lanes (GH #28, R-CG-3)

An agent core (`cogny`) is a **sibling hive** of the talkies, not a cell inside one: one
core, N channel voices. It is reached like a tool and answers like an event, so the
connection is two edges plus one knob.

```json
"override_params": {"talky/dispatcher": {"handoff_tools": ["consult_cogny"]}}
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
round that verbalises it in the channel's own voice. A parent that lets the core ask back
draws one pair more, as the shipped `assistant` does since
[#894](https://github.com/mmeyerlein/meclaw/issues/894): the core's `ask` lane onto this
talky's `in_advice`, and this talky's `reply_to_consult` call -- a handoff beside
`consult_cogny` -- onto the core's `in_turn` (`templates/assistant/README.md` § The
consult edges).

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
one. The surface HAS one, one hop away: its collector runs the ambient recall leg, and
since `5.0.0` the deliberate `memory_recall` call reaches the member's own memory hive,
which declares the tool and answers it -- one hop out of this composite and back.

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
| a fast memory question -- one lookup, a flat tier, a second or two | **the member's memory**, through the `memory_recall` tool it declares and answers (GH #78 / GH #552) | inside the same round, as a tool result |
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

### The sidecar (inline extraction)

The lanes above ASK the memory. This one is WRITTEN TO, and it is the only lane on
which the brain does two jobs in one call: it answers, and in the same response it emits
what the turn carried for somebody else -- the durable memory first, and since GH #605
any other section a receiver has offered. That saves a second inference over the whole window
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
the answer path. A completion whose text carries a ```` ```sidecar ```` block leaves it as
the answer with the block cut out, on to the dispatcher exactly as before, plus **ONE
MESSAGE PER SECTION** on lane `sidecar`, out of the composite -- the curator's own sections
(`window`, `gap`) stay with the curator, and `memory` leaves through it unchanged
([#892](https://github.com/mmeyerlein/meclaw/issues/892)). Everything else passes
untouched -- a round with tool calls belongs to the dispatcher whole, and **without a
block contract in the brain's instructions the splitter is a pure pass-through** -- with
one exception since [#894](https://github.com/mmeyerlein/meclaw/issues/894): the curator's
short block ids -- `[#<12 hex>]`, the bare `#<12 hex>` a history tool answers with (up to the
sixteen digits of an ambiguous id's `candidates`), the full sixty-four-digit block id of a
`history_read` answer, and the window's forms of a released, shortened or expired block; a
free-standing run counts only with a letter a-f in it, so a step number, a colour or an order
number stays -- come out of the prose of every text turn and out of every
string of a section that leaves the composite (a duplex voice speaks `fact`, `context` and
`correction` aloud), because every answer the brain writes reaches its channel through this
cell and an internal id never leaves the colony. The sections routed to the curator keep
theirs -- the splitter knows no section, so the composite names them in its knob
`id_sections` (`["window", "gap", "memory"]`, held to the `./splitter -> ./curator` edge by a
lock) -- and so do the block as written and a tool call's arguments; text without an id
leaves byte for byte. The digest of a round capped at `max_iter` does not come through here:
the collector writes it and takes the ids out itself.

**The block rides beside the answer, not in it** (`5.4.3`,
[#871](https://github.com/mmeyerlein/meclaw/issues/871)). The answer half carries the cut
block as its own body slot, `sidecar_raw`: the block exactly as the model wrote it, fence
included, and only when it was readable -- a malformed block is cut and dropped. The
dispatcher passes the slot on with the answer (since dispatcher 1.2.2), and the collector kept
it beside the answer and showed it with that answer in every later window
(collector 4.4.1, "An earlier answer keeps its block"); since `6.0.0` `./curator` does, up to
its knob `sidecar_max_chars` (GH #889). Before, the window held each
earlier answer without its block, and a model that saw its own answers without one stopped
writing it: measured on a running colony's turns, 38 % of the turns with an earlier answer
in view carried the block, and 98 % once the window showed it. No channel ever receives the
slot: the `answer` leaving the collector carries `messages` only. The splitter's contract
moves to 1.0.3.

**A sentence beside a tool call shows a block too** (`5.4.3`,
[#871](https://github.com/mmeyerlein/meclaw/issues/871)). The text a model writes next to a
consult call leaves through the dispatcher's interim path and stands in the window when the
consult comes back -- often as the only earlier answer there. The round itself still passes
untouched, so that sentence used to stand bare, and on a consult's return the replayed model
wrote the block in 9 of 20 answers. Now a round whose text carries no block leaves with the
knob `nothing_block` -- the memory contract's own form of a turn that carried nothing, one
JSON object -- fenced as ```` ```sidecar ```` on `sidecar_raw`: 14 of 20 in the same
replay. It is shown to the model only; no section lane and no channel receives it, because
nobody annotated that turn. A block the model did write beside the call stays in the text
where it wrote it, and none is laid over it. Empty, the knob shows nothing, as before.

**One fence, sections, and a cell that knows none of them**
([#604](https://github.com/mmeyerlein/meclaw/issues/604), built in
[#605](https://github.com/mmeyerlein/meclaw/issues/605)). The block carries ONE JSON
object and one top-level key per section. The splitter reads the object, and for every
top-level key it emits a message on route `sidecar` with `hop.section` set to that key and
the body `{"messages": [], "section": "<key>", "payload": <the section object>}`. It looks
no section up, validates none against a schema and routes none anywhere: **the edges
downstream distribute on `hop.section`**, so a section this composite has never heard of
travels without a line of code changing here.

**A section names the episode of its turn** (`6.4.1`,
[#941](https://github.com/mmeyerlein/meclaw/issues/941)). Every section carries
`context.session_id` and `context.episode_turn_id`: the id the person's episode of that
turn has in the memory, `<session_id>#<tag>-<index>`, exactly as `turn_write` hands it on.
`./curator` names it on the model call and the `./curator -> ./brain` edge promotes it: the
person's row the call wrote, else the person's latest episode of that round in the session,
so the answer after a tool round or a consult's advice names the turn it answers -- the
latest of the round, so where turns overlap or several people speak in one round, that answer
names the latest person's episode of the round; a session without a person's turn carries it
empty. A consulted `cogny` writes no episodes (`turn_write: "0"`) and stamps no id of its own:
its sections carry the value of the turn that consulted it.
`context.turn_id` is the round's uuid and names no episode. A reader that marks the episode
(the memory hive's `in_affect`, `session_id` + `turn_id`) takes both off the section and
builds nothing itself.

**A section body is an object or a sentence** (`5.2.1`,
[#799](https://github.com/mmeyerlein/meclaw/issues/799)). The two halves of the block
contract asked a model for two different things: the section heading is rendered out of the
schema whoever offered it wrote, and every section `./schemas` offers is a
`{"type": "string"}` -- so `{"fact": "Der Termin ist Dienstag."}` is what a model writing
from the heading produces, and until `5.2.1` that section was dropped. It is the second way
into the silence [#797](https://github.com/mmeyerlein/meclaw/issues/797) closed at the other
end of the same seam: the caller hears a holding sentence and then nothing. So a bare,
non-empty string is a section body now. It travels **wrapped** -- `{"payload": "<string>"}`
-- and not repaired: the wrapper is the body slot the lane declares, and the string inside it
is the one the model wrote, byte for byte. An object still travels exactly as it was written;
**the object form is accepted and no longer advertised.** A top-level key whose value is
neither -- a list, a number, an empty or blank string -- is dropped by name, and
`hop.sidecar_dropped` on the answer half lists them, comma-separated. The blank string is
refused HERE because the receiver refuses it THERE: a section with no words ends as a named
refusal at the far end, not as an advice.

**The legacy fence still reads.** A ```` ```memory ```` block is the single-section form
this cell shipped first, and it becomes the section `memory` with the whole block as its
payload. So does a bare ```` ```json ```` fence or a naked trailing object carrying the
memory payload -- the tolerance the harness grades with. Since `5.4.3` a naked object in the
section form, `{"memory": {...}}`, is read as that section and cut whole: the scan used to
stop at the inner object, flag it malformed and leave `{"memory":` in the answer a reader
saw ([#871](https://github.com/mmeyerlein/meclaw/issues/871), the leak class of
[#534](https://github.com/mmeyerlein/meclaw/issues/534)). Until `talky@5.1.0` the port was
called `extraction` and carried exactly one thing, the raw block as the text of a single
turn; a parent still wired on `hop.route == 'extraction'` writes nothing.

**A block it cannot read leaves the answer too, and that is a retraction**
([#534](https://github.com/mmeyerlein/meclaw/issues/534)). Until then an unreadable block
was left INSIDE the answer and only flagged: half-cutting a block nobody can read, the
reasoning went, corrupts the answer for the sake of a write that cannot happen anyway. It
was measured wrong in a running colony -- a model that had annotated the turn before it
correctly dropped one closing brace, and the raw JSON travelled through the dispatcher and
out to the chat window. There is no half cut to fear: the parser has already located the
span, and the prose either side of it is the same prose whether or not the JSON in the
middle parses. So **`found` decides the cut and READABLE decides the lane** -- an unreadable
block comes out of the answer, `hop.sidecar == "malformed"` records that one was seen, and
NOTHING goes out on `sidecar`, because a block nobody can read is not an annotation
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
 "condition": "has(hop.route) && hop.route == 'sidecar' && has(hop.section) && hop.section == 'memory'",
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
The parent edge was `hop.tool_name == 'remember'` instead of the `sidecar` route above,
and the brain carried a `remember` tool named in `async_tools` (never in
`handoff_tools`: a memory write answers nothing and never comes back, so the
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

**The block IS the contract, and this composite DELIVERS it** (#525, reshaped by
[#606](https://github.com/mmeyerlein/meclaw/issues/606)). The authority is
`templates/memory-hive/inline-contract.md`; what puts it in front of a model is
`./collector`, because the knob `sidecar` is switched on in this composite's own
`collector/config.json` (`override_params` on `assemble`; the default is off, and `cogny`
switches it on too since it grew a splitter, GH #892). *That the collector HOLDS the text and writes it on
every assembly is retracted, not quietly reworded*: the text belongs to whoever reads what
it produces, so the memory hive offers it on `sidecar[]` of its `schemas` answer, the
collector merges the offers of everyone it asked the way it already merges a tool menu
(GH #529), and the composed block is written to `system.instructions.sidecar` on the MENU
message — the same durability class as `system.tools`, one write per change instead of one
per turn. Nothing has to be pasted anywhere, and that is the repair: the
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

**Every knob of this composite is a param now**
-- the collector's since `collector@1.2.0`,
the keeper's since `session-keeper@2.2.0`,
the dispatcher's since `dispatcher@1.2.0`
([#138](https://github.com/mmeyerlein/meclaw/issues/138)).
What stays in `.env` is the provider lane: the API key and the model id. Each row
below names the CELL the knob belongs to, because that is what an
`override_params` entry addresses (GH #140).

| knob | where | default | unit |
|---|---|---|---|
| `idle_ms` | param | `7200000` | session-keeper/close -- silence before a generation may end (2 h) |
| `schedules[0].cron` | param | `0 0,30 22-23,0-3 * * *` | session-keeper/night -- the sweep, **in UTC** (summer image of 00:00-05:30 CEST). A timer has no top-level `cron` param: an override names the whole `schedules` array |
| `close_limit` | param | `50` | session-keeper/close -- generations one firing may seal |
| `max_iter` | param | `8` | collector -- **the loop bound**; at the cap the seam leaves on `answer` |
| `round_idle_ms` | param | `120000` | collector -- idle window of one tool round |
| `memory_tier` | param | `""` | collector -- empty = no memory leg at all |
| `memory_form` | param | `"readable"` | collector -- `readable` / `json` / `both` |
| `turn_write` | param | `"1"` | curator/writer (the collector's until `6.0.0`, GH #889) -- **on by default** (GH #298): one message per unwritten turn leaves on route `turn_write` after every stored turn and every stored answer. `""` or `"0"` switch it off, and off means nothing said in this session reaches a memory at all |
| `role`, `keep_recent`, `compress_at`, `rebuild_to`, `quality_cap`, `horizon`, `tiers`, `summary_budget`, `keep_rounds`, `stub_tools_after`, `short_ids`, `context_window`, `sidecar_max_chars` | param | `role` `"talky"` (GH #892), the rest see [`curator`](../curator/#knobs) | curator/policy -- the window, since `6.0.0` (GH #889), by the talky role's presets since GH #892; the full table is in the curator's README |
| `tools` | param | `["web_search", "web_fetch", "history_search", "history_read", "history_outline"]` | collector -- the tool names this agent **declares** it uses (GH #464). Set at this template since `4.5.0`: a channel voice wants a small, named surface, so the shipped list is two search tools and not `["*"]`, plus the model's own wall, which `./curator` answers inside (since `6.1.0`, GH #893). The schemas behind the names are asked for on the `schemas` lane and written into the brain as `system.tools`; an empty list asks nothing at all. A level that puts a reasoning core beside this surface overrides the list to add `consult_cogny` (GH #529) -- the errand is the level's, not this template's. See [The menu is asked for](#the-menu-is-asked-for-not-typed-schemas--in_menu-gh-464) |
| `max_calls` | param | `16` | dispatcher -- per-answer call budget |
| `async_tools` | param | `""` | dispatcher -- tools that answer on their own lane instead of inside the round, as a JSON array or one comma-separated string. Since `dispatcher@1.2.0` it is a param of THIS composite's own dispatcher (GH #138), so the surface's list and a sibling core's list are two statements and not one. It carried `remember` until `talky@4.1.0`; per-turn extraction is not a tool call any more (GH #379), so the list is empty unless the instance wires an async tool of its own |
| `handoff_tools` | param | `""` | dispatcher -- the tools whose call ends the TURN, because the answer comes back as a later one (`consult_cogny` -- and since GH #530 that is the whole list: `ask_memory` was retired, not replaced). Declares async too -- the dispatcher unions the two lists, so one entry is enough and naming a tool in both is harmless, just redundant. `remember` did not belong here while it existed (GH #372) |

**`ctx.model` is the one instantiation-class knob** and it is strict: `add_nodes` without
it is rejected with `ctx_key_missing`. Two equally valid forms (session ruling
2026-08-15): pass a **resolved literal** (the K-H2 builder convention — the builder
resolves `MODEL_<ROLE>` from `.env` itself), or pass the **`${MODEL_<ROLE>}` token**
verbatim so the cell re-resolves it from `.env` at spawn — the examples use the token
form to stay vendor-neutral. Since `4.3.0` the composite carries exactly ONE `llm` cell,
the brain, so the key has one reader; a subscription or another provider for it is
`override_params` on `brain.params` (`provider`, `auth`, `auth_ref`, `base_url`).

**The brain's backstop is 240 000 ms since 5.4.0.** `brain` carries `cell.message_timeout` 240 000 over its `external_timeout_ms` 120 000. It was 180 000; the wider margin lets an instance whose provider needs a longer call -- a local model with `external_timeout_ms` 180 000 -- keep the rule that the backstop outlasts the call it guards (`crates/meclaw-cells/tests/a_shipped_llm_backstop_outlasts_its_own_call.rs`: at least 10 s and 10 % above) without a template of its own.

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

The composite comes up with all twenty-four cells (plus four hive markers); the two `timer`s
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
`identity` (`brain/config.json:17-23`), and that first slot is where a person -- the user,
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
- **Not a persona.** Identity and instructions live in the brain's `cell.db`, one writer per
  `system` path: since `6.0.0` `./curator` writes the brain and its ledger records whose
  slot each path is -- the collector's, the pack's or its own (GH #889). Since `4.3.0` (GH
  #447) no writer inside this composite owns `system.handover` at all, because there is no
  such slot any more. The topology owns none of that. **Tool schemas moved this line**
  from `4.2.0` (GH #55) to `6.0.0` (GH #889), while the composite implemented
  `thread_recall`; since then it ships none. A tool the parent wires is still entirely the
  agent's, schema included.
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

The brain accordingly ships `params.credential_grant_id` as the empty
string, which is no grant at all (GH #271): standalone this composite behaves
exactly as it did before and spends its `api_key`. Since 5.0.0 that empty string
is a LITERAL and not a `${TALKY_CREDENTIAL_GRANT_ID:-}` token
([#138](https://github.com/mmeyerlein/meclaw/issues/138), ruling R-0904-6): a
grant id is a reference, not material, and two generations in one colony present
different ones -- which an environment variable, being colony-wide, could not
say. Switching it over takes **two**
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
- `crates/meclaw-cells/tests/gh855_an_override_reaches_the_brain_through_its_door.rs` --
  the model door in a running colony: a targeted replacement set at the registry reaches
  this brain through `in_model` with the new model and its prompt block first in the system
  part of the next provider call, clearing it restores the start value, and an edge from
  outside onto `./brain` is still refused.
- `crates/meclaw-cells/tests/talky_composite.rs` -- the shipped template in a running
  colony against the mock OpenAI wire: one turn through session-keeper, seam, brain,
  dispatcher, a tool and back to the seam (two provider calls, the second one carrying
  the tool result, the answer carrying the minted session id and `iter=1`); a close
  whose batch
  reaches the write port and which costs exactly one further provider call, the
  curator's handover note through its own summarizer (GH #896); the next generation's
  first call carries that note as `history.handover`, never the batch.
- `crates/meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs` -- the
  two golden manifests over the instantiated tree (the sub-unit refs produce the same
  bytes the copies did) plus the stamp pin: a cell inside a referenced sub-unit carries
  its OWN template and names `talky` above it.
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
  output forms, run through the shipped `params.script_inline` itself: a cut, a
  byte-identical pass-through (no block, and a tool-call round), the flagged
  pass-through a block nobody can read earns, and -- since GH #605 -- one block with three
  sections leaving as three messages, the one section that can carry nothing dropped by
  name, and -- since GH #799 -- the bare-string section arriving wrapped. `talky_composite.rs`'s
  `an_annotated_answer_splits_into_the_reply_and_the_sidecar` is the same thing end to
  end -- the prose reaches the reply exit fence-free and the section leaves on `sidecar`,
  for ONE provider call.
- `crates/meclaw-cells/tests/gh273_a_swept_close_reaches_the_memory.rs` -- a
  conversation ended the only way this template ever ends one, by a SWEEP, drained
  through the shipped `memory-drain` into the memory hive's real write path: the episode
  rows land with the room and the round of the CONVERSATION, although the sweep that
  ended it knows neither. The same property at the write port is pinned in
  `talky_composite.rs`.
- The sub-units keep their own pins: `session_keeper.rs`, `collector_window.rs`,
  `collector_colony.rs`, `curator_cells.rs`, `dispatcher_template.rs`.
