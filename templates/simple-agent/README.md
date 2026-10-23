# `simple-agent@1.1.2`

A plain agent in hive form, built the way agents are commonly built: a system prompt and
messages, native tool calling, results as `tool_result`, and a loop until the model stops
(capped at `max_iter` 16). Its own tools come from a [`tools`](../tools/) hive; a caller adds
**client tools** per turn, which leave the hive as `calls` and come back as `tool_result`
inside the same turn. Identity and relationships live in the agent's own
[`affinity`](../affinity/) hive. A [`curator`](../curator/), a [`memory-hive`](../memory-hive/)
and the sidecar work as in `talky`/`cogny`.

Sealed (`params.ports: []`), its own [`access`](../access/) with sealed vault grants, the
`in_model` door. One instance per agent; a caller that needs a world, a phone or a channel
wraps the agent and speaks its turn door.

## Cells

| path | type | from |
|---|---|---|
| `door` | `code`, resident | the turn door (contract below), the identity boot, the client calls and their watchdog: the only Python of this template |
| `collector` | ref | [`collector`](../collector/): `max_iter` 16, `fold_tools` on, `round_idle_ms` 120000, the tool names spelled out (no `*`: that would bring the build pair) |
| `watch` | `timer` | one one-shot schedule per round that sent a client call (below) |
| `curator` | ref | [`curator`](../curator/), role `talky`, its summarizer on its own grant |
| `brain` | `llm` | the one thinking cell: `${ctx.model}`, `reasoning_effort` unset |
| `splitter` | `code` | as in `cogny`: the sidecar block, sections `window`, `gap`, `memory` |
| `dispatcher` | ref | [`dispatcher`](../dispatcher/): no async or handoff tools |
| `memory-hive` | ref | [`memory-hive`](../memory-hive/) |
| `access` | ref | [`access`](../access/), seeded with the grants of the seven model consumers |
| `tools` | ref | [`tools`](../tools/): `bash`, `file`, `edit`, `web_fetch`, `web_search`, `weather` -- no `build_topology`/`apply_manifest` |
| `affinity` | ref | [`affinity`](../affinity/): the agent's identity and relationship record |

## The turn door (contract V-T)

Every rim lane carries the body slot of its own name.

| lane | direction | hop | body |
|---|---|---|---|
| `turn` | in | `turn_id` (required), `session_id`, `channel`, `audience_set` (a JSON array of participant ids) | `messages` (meclaw turns, or `{role, content}`), optional `tools[]`, optional `tool_scope: {require: true}` |
| `tool_result` | in | the five round keys of its `calls` message: `turn_id`, `session_id`, `iter`, `channel`, `audience_set` | `{id, content, is_error?}` |
| `close` | in | -- | `{session_id}` |
| `in_pack` | in | -- | `system.*` slots for the brain (`identity`, `persona`, `handover`, `instructions`), e.g. an instance's standing instructions |
| `in_model` | in | `subscriber` | a model package (llm-registry) |
| `mutation_committed` | in | -- | the collector asks for its tools again |
| `answer` | out | `turn_id` | `{turn_id, text, partial?, stale?}` |
| `calls` | out | `turn_id`, `session_id`, `iter`, `channel`, `audience_set`, `tool_name`, `tool_call_id` | `{id, name, arguments}`, one message per client call |
| `close_report` | out | -- | `{session_id, ok, added, closed}` |
| `model_refused` | out | `refused_subscriber` | a model package a model cell refused |
| `error` | out | `turn_id` where there is one | `{error_code, ...}` |

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

**One caller per agent, turns in series.** The menu is state of the one brain: two callers
with different client tools would see each other's menus. Several channels of one owner
share the agent one turn at a time. The door holds it: a `turn` that arrives while another
turn waits on a client call is refused with `error {error_code: "turn_busy", turn_id,
open_turn}`, and the open turn's menu row stays as it is.

**The loop.** The brain calls tools until it answers in text. At `max_iter` (16, the
collector's cap) the collector stops the loop and the answer carries `partial: true`; the
composite caps the curate seam at `iter < 20` as well, like `cogny`.

## Identity (contract V-A)

The agent's identity is one AIeOS 1.1.0 document in its own affinity hive, and it reaches the
brain only as affinity's pack. At the first message after boot the door writes, in order and
each after the previous `ack`:

1. `upsert_entity` `{entity_id: "entity:<agent_id>", kind: "agent", display_name, aieos}` --
   affinity's gate checks the AIeOS paths it requires;
2. one `set_disclosure` per path of the door param `disclose` (default `["aieos"]`) to
   `agent:<agent_id>` -- affinity's brief releases nothing without one (fail-closed);
3. `subscribe {subject: "entity:<agent_id>", slots: ["identity"]}` with the subscriber token
   `self-brain` fixed in the template.

Affinity's push then sends the identity pack to the curator's `in_pack` (the edge compares
`hop.subscriber == 'self-brain'`), and the curator's `pack_ack` books the delivery back in
affinity. Every later change of the record sends a new pack. Turns that arrive before the
first pack wait for it; a refused write ends the boot with `error
{error_code: "identity_refused"}` and the turns go on without the identity. Without an `aieos`
document there is no identity boot. The start memories (`start_memories`) are written as
episodes into the memory hive under the audience `["*"]`, so every caller's recall can find
them. The actor of every write is `agent:<agent_id>`, set on the
edge, never taken from a body.

**Relationships.** The agent writes its own record through the tool `affinity_propose`
(`op` one of `add_relation`, `retract_relation`, `set_trust`, `propose`, `upsert_entity`); its
`ack` is the tool's result. Nothing else in the core proposes to affinity.

Affinity ships its placeholder seed (Alex, Robin, Sam, Aiden, with one inactive subscription
for `aiden`); it is harmless, but do not name an agent `aiden`.

## Instance params

Set through `override_params` on the cell paths of the instance (`<agent>/door`, ...). There
is no deep merge: overriding `collector` replaces only the top-level key named.

| cell | param | default | meaning |
|---|---|---|---|
| `brain` | `model` | `${ctx.model}` | the provider model id, a resolved literal |
| `brain` | `reasoning_effort` | unset | `low`/`medium`/`high`, passed through |
| every model cell | `x_title`, `http_referer` | `${OPENROUTER_X_TITLE:-MeClaw}`, `${OPENROUTER_HTTP_REFERER:-https://meclaw.ai}` | the app attribution every request sends |
| `door` | `agent_id` | `agent` | the agent's id: entity `entity:<id>`, actor `agent:<id>` |
| `door` | `aieos` | `{}` | the AIeOS 1.1.0 document of the agent |
| `door` | `disclose` | `["aieos"]` | the field paths the agent's own brain may see |
| `door` | `start_memories` | `[]` | `{what, with?, at?}` written once as episodes |
| `door` | `round_idle_ms` | 120000 | the watchdog's wait; keep it equal to the collector's |
| `door` | `tools_allow`, `tools_deny` | `[]` | the tool scope of every turn; **an allow list also filters client tools** |
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
| `affinity/brief`, `affinity/gate`, `affinity/push` | the identity boot after a restart and an `affinity_propose` -- no edge of a turn reaches affinity |
| `memory-hive/close-glue`, `memory-hive/entity-glue` | the close of a session and its entity pass, in a figure once a night |

The cells that run rarely in every consumer ship `cold` from their own template since GH
#1294 and need no marker here: `curator/schemas`, `memory-hive/schemas`, `tools/schemas`
(answered when the collector asks for its tools, at boot and after a mutation),
`memory-hive/dream-glue`, `memory-hive/recur` (the dream run), `access/policy`, `access/sweep`,
`curator/stats` (a grant decision, the TTL timer, an observer's `in_stats`).

Every other code cell keeps the mode its template ships; `access/invoke` stays warm because a
model cell waking after its own idle asks it for the credential before the first call.

## Limits

- No world, no game day, no phone, no companion: a caller that needs one wraps the agent.
- No build pair: an agent that changes topology is a different template.
- One caller per agent (the menu is the brain's state); turns in series, an overlapping turn
  is refused as `turn_busy`.
- The door's state is RAM and lives as long as its child: the boot gate (turns wait for the
  first identity pack; a pack that never comes and never fails holds them, there is no boot
  watchdog), the answered turns (deduplicated by `turn_id` alone, so a caller sends unique turn
  ids across sessions), and the start-memory flag (a respawned child writes the start memories
  again; the memory's close pass folds the duplicates).
- The collector's iteration cap is the only loop guard; a model that never stops answers
  `partial` at 16.
