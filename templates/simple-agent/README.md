# `simple-agent@2.2.4`

Version 1 was a sealed agent with its own affinity, its own access and fixed grants; that
line is frozen as the max-agent (`simple-agent@1.1.4`, its own directory beside this one).
Version 2 is the agent of a colony: affinity, access and the llm-registry belong to the
colony and reach the agent over contract lanes its embedder draws, so the agent keeps no
record of people and no key of its own.

A plain agent in hive form, built the way agents are commonly built: a system prompt and
messages, native tool calling, results as `tool_result`, and a loop until the model stops
(capped at `max_iter` 16). Its own tools come from a [`tools`](../tools/) hive; a caller adds
**client tools** per turn, which leave the hive as `calls` and come back as `tool_result`
inside the same turn. A [`curator`](../curator/), a [`memory-hive`](../memory-hive/) and the
sidecar work as in `talky`/`cogny`.

Sealed (`params.ports: []`): the colony sees lanes, never cells. One instance per agent; a
caller that needs a world, a phone or a channel wraps the agent and speaks its turn door. A
role is this agent plus the role's own code around it.

## Cells

| path | type | from |
|---|---|---|
| `door` | `code`, resident | the turn door (contract below): the queue, the affinity tool's writes, the client calls and their watchdogs, the usage of a turn: the only Python of this template |
| `door_store` | `store` | the door's durable state: the queue, the open turn, the answered turn ids, the start-memory mark, the open client calls |
| `collector` | ref | [`collector`](../collector/): `max_iter` 16, `fold_tools` on, `round_idle_ms` 120000, `brief_slots` `["peer", "channel"]`, the tool names spelled out (no `*`: that would bring the build pair) |
| `watch` | `timer` | one one-shot schedule per round that sent a client call (below) |
| `curator` | ref | [`curator`](../curator/), role `talky` |
| `brain` | `llm` | the thinking cell of a turn: `${ctx.model}`, `reasoning_effort` `medium` |
| `strong` | `llm` | the second brain of a turn marked `stage: strong`; asked only when the door's `strong` param names it |
| `splitter` | `code` | as in `cogny`: the sidecar block, sections `window`, `gap`, `memory` |
| `dispatcher` | ref | [`dispatcher`](../dispatcher/): no async or handoff tools |
| `memory-hive` | ref | [`memory-hive`](../memory-hive/) |
| `tools` | ref | [`tools`](../tools/): `bash`, `file`, `edit`, `web_fetch`, `web_search`, `weather` -- no `build_topology`/`apply_manifest` |
| `tool_gate` | `code` | the own tool families by enablement (below): the menu ask to `./tools` and every own tool call pass it |
| `enrol` | `code`, cold | the agent's enrolment at the colony's affinity on its own commit receipts (below) |

No `access` and no `affinity`: both are the colony's.

## The turn door (contract V-T)

Every rim lane carries the body slot of its own name.

| lane | direction | hop | body |
|---|---|---|---|
| `turn` | in | `turn_id` (required), `session_id`, `channel`, `audience_set` (a JSON array of participant ids), `counterpart` (who the turn is with, the subject of its brief) | `messages` (meclaw turns, or `{role, content}`), optional `tools[]`, optional `tool_scope: {require: true}` |
| `tool_result` | in | the five round keys of its `calls` message: `turn_id`, `session_id`, `iter`, `channel`, `audience_set` | `{id, content, is_error?}` |
| `close` | in | -- | `{session_id}` |
| `in_pack` | in, required, `at` `./curator` | -- (context `pack_sub`, `pack_hash` from the embedder's edge) | the identity and `instructions` from the colony's affinity, `system.*` slots for the brain (`identity`, `persona`, `handover`, `instructions`) |
| `in_model` | in, required | `subscriber` | a model package (llm-registry) |
| `in_sealed` | in, required, `at` the eight model cells (`./strong` among them) and `./tools/web_search` | `grant_id` | the colony access's sealed box for the cell that asked |
| `in_briefing` | in, required, `at` `./collector` | `brief_outcome` | the colony affinity's brief (or its error) for this turn's counterpart |
| `in_propose_ack` | in | `propose_outcome`, `outcome` | the colony affinity's answer to one `affinity_propose` |
| `mutation_committed` | in | -- | the collector asks for its tools again |
| `in_conversations` | in | -- | `{reader, audience_set, session_id? \| channel? + since?, max_chars?}`: a reader asks this agent's memory for whole conversations (the memory hive's lane, unchanged) |
| `answer` | out | `turn_id` | `{turn_id, text, partial?, stale?}` |
| `calls` | out | `turn_id`, `session_id`, `iter`, `channel`, `audience_set`, `tool_name`, `tool_call_id` | `{id, name, arguments}`, one message per client call |
| `close_report` | out | -- | `{session_id, ok, added, closed}` |
| `model_refused` | out | `refused_subscriber` | a model package a model cell refused |
| `error` | out | `turn_id` where there is one | `{error_code, ...}` |
| `conversations` | out | `reader` | `{reader, sessions: [{session_id, channel, episodes: [{episode_ref, role, text, ...}]}], truncated, withheld}` or `{reader, error_code}`; only sessions on conversation channels, never `role`, `ask` or `inner` |
| `credential_request` | out, `at` the eight model cells (`./strong` among them) and `./tools/web_search` | `grant_id` | a cell born with a grant asks for its key |
| `pack_ack` | out | `error_code`, `pack_slots` (context `pack_sub`, `pack_hash`) | the receipt of an `in_pack` |
| `brief` | out | `turn_id` (context `audience_set`, the round) | `{subject, channel, slots}` about the turn's counterpart |
| `affinity_propose` | out | `agent_id`, the round keys, `tool_call_id` | one write of the affinity tool, as affinity's `in_propose` takes it |

**Client tools.** `tools[]` of a turn is a list of provider schemas, `{name, description,
parameters}` or the OpenAI form `{type: "function", function: {...}}`. They are the client's
menu for **this turn only**: the door writes them as the menu row of answerer `client` before
the turn opens, and a turn without `tools[]` empties that row. A client tool whose name the
agent already owns (one of the six families, `memory_recall`, `history_*`,
`affinity_propose`) is dropped, and the turn carries `client_tools_dropped` (a JSON array of
the names) on its hop -- the menu merges rows by answerer, and `client` sorts first, so the
caller's tool would otherwise replace the agent's own.

**A turn can require a tool call.** `tool_scope: {require: true}` on a turn asks the turn's
**first** brain call for a tool call (the `llm` cell's `tool_scope.require`: `tool_choice:
"required"` on the chat wire, `{"type": "any"}` on the Anthropic wire). The door stamps
`tool_require: "1"` on its `in_turn`, and the collector adds `require` to the scope of that
one call only: the next calls of the same loop go free, so the turn still ends in text and
`max_iter` stays the bound. Only `require` is read; the scope of the turn stays the door's
`tools_allow`/`tools_deny`. A turn without `tool_scope` is unchanged.

**A client call does not end the turn.** It runs the synchronous path of a tool call: the
collector expects its id, the door sends it out on `calls`, and the matching `tool_result`
continues the same turn. **The caller returns the five round keys of the `calls` hop
unchanged** on the `tool_result` hop: the collector files a result under the turn, the session
and the iteration it was asked in, and the door refuses a result without them as
`error {error_code: "tool_result_unkeyed"}` instead of parking it in a round nobody opens.
`is_error: true` is filed as a tool result that starts with `error: `.

**A result that never comes ends the turn named, not silent.** The collector sweeps a stuck
round only when asked, so the door arms one one-shot schedule at `./watch` per round that
sent a client call, `round_idle_ms` + 1 s ahead. When it fires and the result is still
missing, the collector files `tool result lost` under the call's id, the brain answers with
that in its window, and the answer carries `stale: true`. A schedule that fires after the
result finds no open round and does nothing. **Set `round_idle_ms` (door and collector, the
same value) at or above the caller's own deadline for a client call.**

**Several callers, one turn at a time (the queue).** The door takes the turns of several
callers and runs one to its end before the next begins: one round, one window, one menu row
of client tools. A turn that arrives while another runs is queued, never refused; the next
to begin is the queued one of the lowest `hop.priority` (0 first, default 5), then the
earliest. `hop.deadline_ms` (epoch milliseconds, optional): a turn that has not begun by then
ends as `answer {turn_id, expired: true}` without a model call -- also while another turn
still runs: one one-shot watchdog of the door, re-armed to the earliest deadline in the queue,
wakes it at that moment (no tick). Every message back to a caller
carries the turn's `hop.caller` as the rim edge stamped it on the turn, so an embedder routes
the answer home by it. A `turn_id` is answered once: a repeat within seven days is refused
with `error {error_code: "turn_seen", turn_id}`.

**The door's state is durable.** The queue, the open turn, the answered turn ids (seven days),
the start-memory mark and the open client calls live in `./door_store` (a `code` cell has no
`cell.db`). A new child life of the door reads them back before it serves its first message,
so a restart loses no queued turn, writes the start memories no second time and answers no
turn id twice. A read the store does not answer within `store_read_ms` is asked again.

**The answer says what the turn cost.** `answer` carries `iters` (the brain calls of the
turn), `usage {in, cached_in, cache_write, out}` and `cost`, summed from the `hop.tokens_*` and
`hop.cost` of every brain (or `strong`) answer of the turn, and `stage` (`fast` or `strong`,
the stage that ran).

**Required tools.** `hop.required_tools` (a JSON array of tool names): when the turn's answer
comes without a call of one of them, the door sends exactly one follow-up turn in the same
session ("Call <name> now.", the turn's client tools in the menu again); its answer is the
turn's answer, with `missing_required: [...]` if a tool is still missing. The door checks
this in code: `tool_scope.require` has no effect on a native Anthropic model.
`hop.required_mode` chooses what counts: `all` (the default, also when absent) requires every
named tool; `any` is satisfied by a call of one of them. In mode `any` a turn that hit the loop
cap (`partial`) gets no follow-up (its budget is spent; `missing_required` still names the
tools), the follow-up's first model call must call a tool (`tool_choice: "required"`), and the
answer keeps the first text before the follow-up's (joined by a blank line). `hop.required_ask`
replaces the follow-up's words ("Call <name> now.", in mode `any` "Call one of <a>, <b> now.").
The follow-up runs as round `<turn_id>.follow`, so its client calls carry that `turn_id`; the
answer carries the turn's own.

**Client calls have a deadline.** A client call unanswered after `client_tool_timeout_ms`
(door param, default 30000) becomes the result `error: timeout: ...` in its round, and the
loop goes on; a result that arrives after its timeout is dropped.

**Two stages.** `hop.stage: strong` runs the turn on `./strong` when the door's `strong` param
is set, else on `./brain` and the answer says `stage: fast`. Both share the collector, the
curator, the memory and the identity; the curator keeps each stage's package (`input_soft`,
`input_hard`) from the answers of that stage's model (`context.sa_stage`) and sizes a call by
the package of its own stage, never by the other stage's last answer. Before a stage's model
has answered once, its first call takes the last answer's package.

**The loop.** The brain calls tools until it answers in text. At `max_iter` (16, the
collector's cap) the collector stops the loop and the answer carries `partial: true`; the
composite caps the curate seam at `iter < 20` as well, like `cogny`.

**A turn can narrow its own cap.** `hop.max_iter` (a whole number, optional) on a turn sets
the loop cap of THAT turn, counted like the collector's `max_iter` (tool rounds; N rounds are
N + 1 brain calls), so an embedder spends a short budget on one errand without retuning the
agent. It only narrows: the collector's `max_iter` stays the default and the upper bound, and a
value at or above it, zero, negative, a fraction (`2.7`) or not a number is the param, without an error (the param
is the operator's ceiling; a caller asking for more has asked for nothing the agent may grant).
The door stamps it as `turn_max_iter` on its `in_turn` (and on the turn's required-tools
follow-up), the collector writes it into the round's own row, and the row falls with the round:
the next turn starts from the param again.

## The colony's lanes

**Credentials.** Seven cells spend a provider key: `brain`, `curator/summarizer`,
`memory-hive/closer`, `memory-hive/dialectic`, `memory-hive/dreamer`, `memory-hive/judge`,
`memory-hive/embed`. None is born with a grant. The embedder seeds one grant per consumer in
the colony's [`access`](../access/) and names it at birth through `override_params`
(`credential_grant_id` and `api_key` are immutable, so a message cannot set them):

```json
"override_params": {
  "brain": {"api_key": "", "credential_grant_id": "grant:example-provider@colony/agent-brain"},
  "memory-hive/closer": {"credential_grant_id": "grant:example-provider@colony/agent-closer"}
}
```

A cell with a grant asks on `credential_request` and waits for `in_sealed`; a cell without
one spends its `api_key` (empty: no key at all). One grant per provider is enough: a native
Anthropic brain (`provider: "anthropic"`) takes a grant on `cred:anthropic` like any other.
`tools/web_search` ships anonymous; an embedder that names a grant on it (at birth, like a
model cell) draws the same pair to `./tools/web_search`, which this rim and the tools hive open.

**Identity.** The agent's identity is its entry `entity:<agent_id>` in the colony's
[`affinity`](../affinity/). Affinity's push sends the pack of the subscription to the agent's
curator (`in_pack`, a v-lane at `./curator`), and its `pack_ack` leaves the rim with `pack_sub` and
`pack_hash`, so affinity books the delivery. Turns never wait for an identity: until the first
pack the agent speaks without the identity slot. The door writes nothing at boot except the
frame (`instructions.frame`), the affinity tool's menu row and the start memories.

**Enrolment.** The agent enrols itself; the enrolment is the agent's, not its embedder's. A
manifest sends no message, and `subscribe` takes its subscriber from the edge, so `./enrol`
does it on a commit receipt (`mutation_committed`) that concerns the agent: the boot's
(`form: boot`), or one whose `paths` name a node at, under or above the agent (it was born or
lifted with that node -- a `figure` grown by one `add_nodes` at the member carries its agent
one level deeper) or whose `scopes` name one at or under it (its own graph was written). Both
keys are on every mutation receipt since GH #1312 (`docs/meclaw-overview.en.md`, receipt). A
sibling's birth, a builder block elsewhere and the colony's container write nothing of the
agent's: each `subscribe` books a new row and pushes a pack. A receipt without the two keys (a
colony before GH #1312) keeps the reading before: every manifest, and a single mutation whose
`scope` is the agent, lies under it, or is the level it was born into. The steps, one after the acknowledgement
of the one before: `upsert_entity` (`entity:<agent_id>`, kind agent, the AIeOS document of
`enrol/identity`, `mx.brain` from `enrol/charter`), one `set_disclosure` per `enrol/disclose`
entry (default `aieos` to the agent itself; `mx.brain` too with a charter), `subscribe` with
`slots: ["identity"]` and, with a charter, a second one with `slots: ["brain"]` (affinity
refuses both slots in one request). The steps leave the rim on `affinity_propose` with
`context.sa_enrol` naming the step and come back on `in_propose_ack`, which the rim hands to
`./enrol` rather than the door; the embedder's edge stamps the subscriber `agent:<agent_id>`
for `subscribe` and `agent:<agent_id>/brain` for `subscribe_brain`. Every op is an upsert or
a newest-wins append, so a receipt that comes again writes the same rows again. A refused step
leaves the rim as `error` with `enrol_refused` (`step`, `reason_code`), and the turns go on.
An empty `enrol/identity` enrols nothing.

**Brief.** With `brief_slots` set (`["peer", "channel"]`) and a turn that names a
`counterpart`, the collector asks `brief` once at the turn's opening, with the turn's round as
`context.audience_set`, and waits for `in_briefing`. The colony's affinity refuses a brief
without a round (`no_round`): the leg parks empty and the turn goes on without a brief. A
turn without a counterpart asks nothing.

**Relationships.** The agent writes its own entry through the tool `affinity_propose` (`op`
one of `add_relation`, `retract_relation`, `set_trust`, `propose`, `upsert_entity`). The call
leaves the rim on `affinity_propose` with `agent_id` and the round on its hop; the embedder's
edge stamps the actor `agent:<agent_id>` and sends it to the colony affinity's `in_propose`.
The answer comes back on `in_propose_ack` and is the tool's result in its round; a rejected
op or an affinity error reads `error: ...`, never silence.

The start memories (`start_memories`) are written as episodes into the memory hive under the
audience `["*"]`, once per life of the door's child, so every caller's recall can find them.

## Embedding one level deeper

Every level the agent crosses -- a figure around the agent, an orga level around a
coordinator, a role app around an agent -- carries the same lanes on its own rim with the
same `at` declarations, prefixed by the path of the agent inside it (`./simple-agent/brain`
for a level that holds the agent at `./simple-agent`). A level that declares a lane without
the matching `at` is a mandatory hop (`v_lane_mandatory_hop`); a sealed level without the lane
refuses the edge (`hive_port_boundary`). The embedder at the colony's level draws the edges.

| lane | direction | `at` / inner target | required | what the embedder's manifest draws |
|---|---|---|---|---|
| `credential_request` | out | `./brain`, `./strong`, `./curator/summarizer`, `./memory-hive/closer`, `./memory-hive/dialectic`, `./memory-hive/dreamer`, `./memory-hive/judge`, `./memory-hive/embed`, `./tools/web_search` | -- | one edge per consumer to the colony's access (`lane: credential_request`, route `in_invoke`, `context.requester` stamped) |
| `in_sealed` | in | the same nine | yes | one edge per consumer from the colony's access (`lane: in_sealed`, `ack` of `vault.deliver` addressed by `hop.grant_id`), plus the grant rows and the `override_params` of the grant ids |
| `in_pack` | in | `./curator` | yes | the colony affinity's push for the agent's subscriber token, `pack_sub`/`pack_hash` promoted into context |
| `pack_ack` | out | the rim, from `./curator` | -- | back to the colony affinity's `in_pack_ack` |
| `brief` | out | the rim, from `./collector` | -- | to the colony affinity's `in_brief`, `asker` and `audience_set` stamped |
| `in_briefing` | in | `./collector` | yes | affinity's `answer` and `error` back, `brief_outcome` stamped |
| `in_model` | in | the rim, by `hop.subscriber` to `./brain`, `./strong` (subscriber ending `/strong`), `./curator`, `./memory-hive` | yes | the colony llm-registry's pushes |
| `model_refused` | out | the rim | -- | back to the llm-registry's `in_refused` |
| `affinity_propose` | out | the rim, from `./door` | -- | to the colony affinity's `in_propose`, `actor` stamped from `hop.agent_id` |
| `in_propose_ack` | in | the rim, to `./door` | -- | affinity's `ack` and `error` back, `propose_outcome` stamped |
| `in_note` | in | `./curator` | -- | notes for the window (`notes[]`, curator `in_note`), with the round in `context.audience_set` |
| `in_conversations` / `conversations` | in / out | announced (A4): to `./memory-hive` | -- | the conversations a reader may see |
| `affinity_propose` with `context.sa_enrol` | out | the rim, from `./enrol` | -- | the same edge as the affinity tool's; its `subscriber` is `agent:<agent_id>` for the step `subscribe`, `agent:<agent_id>/brain` for `subscribe_brain`, empty otherwise |

The required lanes are checked at birth (`contract.accepts[].required`): a mutation that grows
the agent without one of them is refused with `hive_contract`. For a lane with `at`, one edge to
any of its targets satisfies the check; that every grant consumer gets its own edge is the
embedder's part, not the birth's.

## The builder block

One block grows the agent: the builder's recipe `simple_agent` (`templates/builder`) renders
the node, every edge of the table above between the agent and the `./access`, `./affinity`
and `./llm-registry` of the level it is born into, the grants in that level's
`./access/store` (`seed_rows`) and the override params, in one declaration at the level:

```json
{"recipe": "simple_agent", "params": {
  "scope": "/", "name": "agent", "agent_id": "ava", "model": "example-provider/brain-model",
  "credential": {"cred_ref": "cred:openrouter", "subject": "colony:example",
                 "expires_at": "2099-01-01T00:00:00.000000Z"},
  "identity": {"…": "an AIeOS document"}, "tools_enabled": ["weather"],
  "app_id": "meclaw-example simple-agent-colony"}}
```

`model` is the brain's start value (`ctx.model`); `strong` names the second brain's and
switches it on at the door; `identity`, `charter` and `disclose` go to `./enrol`,
`start_memories` to the door, `tools_enabled` to `./tool_gate`, and `app_id`/`referer` to
`x_title`/`http_referer` of every model cell: both brains, the curator summarizer, the four memory
cells and the embedder (GH #1315). One grant per consumer of the credential
table (`tools/web_search` only with a `search_cred_ref`). The model road: the registry's
pushes under the agent's path arrive as `in_model`, refusals go back as `in_refused`, and every
receipt of the level announces `brain` (and `strong`) with their need. No runner mode per tool
family: the own families are built-in cell types without a runner. Worked through in
[`examples/simple-agent-colony`](../../examples/simple-agent-colony/).

## Instance params

Set through `override_params` on the cell paths of the instance (`<agent>/door`, ...). There
is no deep merge: overriding `collector` replaces only the top-level key named.

| cell | param | default | meaning |
|---|---|---|---|
| `brain` | `model` | `${ctx.model}` | the provider model id, a resolved literal |
| `brain` | `reasoning_effort` | `medium` | `low`/`medium`/`high`, passed through |
| every model cell and `memory-hive/embed` | `x_title`, `http_referer` | `${OPENROUTER_X_TITLE:-MeClaw}`, `${OPENROUTER_HTTP_REFERER:-https://meclaw.ai}` | the app attribution every request sends (the embedder's since GH #1315) |
| `door` | `agent_id` | `agent` | the agent's id: entry `entity:<id>` in the colony's affinity, actor `agent:<id>` |
| every model cell | `credential_grant_id`, `api_key` | `""` | the grant in the colony's access, set at birth (above) |
| `door` | `start_memories` | `[]` | `{what, with?, at?}` written once as episodes |
| `door` | `round_idle_ms` | 120000 | the watchdog's wait; keep it equal to the collector's |
| `door` | `client_tool_timeout_ms` | 30000 | how long a client call waits for its result before it is a named timeout |
| `door` | `store_read_ms` | 10000 | how long a new child life of the door waits for `./door_store` to answer its read before it asks again |
| `door` | `strong` | `""` | the model (or registry subscription) of `./strong`; empty = no second brain. Set `strong`'s `model` (or its `in_model` subscription) to the same |
| `door` | `tools_allow`, `tools_deny` | `[]` | the tool scope of every turn; **an allow list also filters client tools** |
| `enrol` | `agent_id`, `identity`, `charter`, `disclose` | `agent`, `{}`, `{}`, `["aieos"]` | the enrolment (above); keep `agent_id` equal to the door's |
| `tool_gate` | `tools_enabled` | `[]` | the own tool families on offer: `bash`, `file`, `edit`, `web` (web_fetch, web_search), `weather`; none by default. A family not named is off the menu and a call of it never reaches `./tools`: it goes to the door like any name the agent does not serve, so a client tool of the turn by that name (the caller's `bash`) leaves on the rim. Each menu tick tells the door the enabled families (lane `tools_enabled`); the core tools (`memory_recall`, `history_*`) are never touched |
| `collector/assemble` | `max_iter`, `round_idle_ms` | 16, 120000 | the loop cap; the idle window of a round |
| `tools/file`, `tools/edit` | `base_path` | `workspace` | the file root, a directory of this agent's tools hive; set both to the same value to move it |

The file tools are rooted per instance: the default `workspace` is a directory of the agent's
own tools hive, created at spawn. `bash` runs under the tools hive's sandbox settings, not in
that directory.

## Runner modes

The ref markers set the `code` cells that a turn passes elsewhere but this agent calls only
rarely to `runner_mode: "cold"` (`override_params`; `cell.idle_timeout_ms` sits in the `cell`
block, which a marker cannot reach). A warm child of each would stand 30 minutes after its
last job (GH #1276), and in a figure they were most of its idle processes. Each holds no cache
(warm answers like cold), so cold changes only that every message starts its own process.

| cells | why they run rarely here |
|---|---|
| `memory-hive/close-glue`, `memory-hive/entity-glue` | the close of a session and its entity pass, in a figure once a night |
| `enrol` | a commit receipt and the acknowledgements of its steps; ships `cold` from this template |
| `memory-hive/sessions` | a reader's question (`in_conversations`) or the session tier of `in_query`; ships `cold` from its own template, no marker |
| `tools/weather`, `tools/build-draft`, `tools/build-apply` | every family is off by default, and `weather` is the one family cell that is a `code` cell (the others are built-in cell types with no runner mode); the build pair this agent never offers. An embedder that enables `weather` sets `tools/weather` back to `warm` in the same block |

The cells that run rarely in every consumer ship `cold` from their own template since GH
#1294 and need no marker here: `curator/schemas`, `memory-hive/schemas`, `tools/schemas`
(answered when the collector asks for its tools, at boot and after a mutation),
`memory-hive/dream-glue`, `memory-hive/recur` (the dream run), `curator/stats` (an observer's
`in_stats`).

Every other code cell keeps the mode its template ships.

## Limits

- No world, no game day, no phone, no companion: a caller that needs one wraps the agent.
- No build pair: an agent that changes topology is a different template.
- Turns run one at a time. Turn ids are deduplicated by `turn_id` alone, so callers send
  unique turn ids.
- The proposals waiting for affinity's answer are RAM and live as long as the door's child.
- The collector's iteration cap is the only loop guard; a model that never stops answers
  `partial` at 16, or at the turn's own `hop.max_iter` when that is lower.
