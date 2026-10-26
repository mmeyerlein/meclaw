# `figure@2.0.2`

A character in a shared world, built as an onion: inside, an unchanged
[`simple-agent@2`](../simple-agent/), the agent of a colony (one model behind a curator, a
memory, the turn door; affinity, access and the llm-registry are the colony's); around it, two
adapters. `world` speaks the world's lanes, `phone` speaks to a
colony exactly the way a human channel does. The world stands beside the figure, never inside
it: it renders the situation and hands its tools to each turn as client tools, and the figure
calls them like any other tool until the model stops.

One instance per character. There is no figure without a colony: each figure lives in a
colony whose `access`, `affinity` and `llm-registry` serve its agent, and the colony draws the
agent's contract lanes to the figure's rim (below).

## Cells

| path | type | what it does |
|---|---|---|
| `world` | `code`, resident | the world adapter and the ONLY sender into the agent's turn door: every turn (world, phone, interview) runs through it, one at a time |
| `phone` | `code`, warm | the handset: turns a message of the figure into a human-channel `turn`, and an `answer` on that channel into a phone turn |
| `simple-agent` | ref [`simple-agent@2`](../simple-agent/) | the agent itself, referenced, never copied |

No model cell and no memory of its own: both are the agent's. No grant, no `access` and no
`affinity`: those are the colony's, in the figure as in the agent. The frozen 1.x
line of `simple-agent` (the max-agent with its own access and affinity) is not what a figure
runs.

## Contract with the world (V-W)

Every lane carries the body slot of its own name.

| lane | direction | body |
|---|---|---|
| `obs` | in | `{turn_id, kind, text, tools[], deadline_ms?, day, at?}`: one turn. `at` is the world's game time (ISO-8601); the phone stamps the last one on what it sends. `tools[]` (provider schemas `{name, description, parameters}`) is the world's menu for this turn only; `text` is the situation as the world renders it |
| `call` | out | `{turn_id, call_id, tool, args}`: the figure called a world tool during the open turn |
| `call_result` | in | `{turn_id, call_id, result}`: the world's answer to one `call`. `result.accepted == false` reaches the model as an error result; anything else as a plain result |
| `intent` | out | `{turn_id, done: true, text}` when the model stopped (its final text; the speech of a `contact` turn), `{turn_id, never: true}` when the turn failed |
| `night` | in / out | in `{day}`: closes every session the figure used that day (`day-<day>`, `phone-<day>`, `interview-<day>`); out `{day, ok, added, closed}` after the LAST close report, the sums over all of them |
| `interview` | in / out | in `{question_id, question}`: a turn on channel `interview:<world_id>` without tools; out `{question_id, answer}`, or `{question_id, answer: null, error, error_code}` |
| `mandate` | in | `{action: start\|stop, until?, frame?, note?}`: the pack leaf `instructions.mandate` of the agent until a `stop` |
| `phone_capped` | out | `{day, channel, text, replies, max}`: an answer on the handset over `phone_replies_max` of the world day; it opened no turn (see the handset) |
| `error` | out | `{error_code, turn_id?, kind}`; `error_code` is never null |
| `calls` | out | `{id, name, arguments}`, hop `tool_name`, `tool_call_id` and the round keys: the model called one of the instance's `extra_tools` (below) |
| `tool_result` | in | `{id, content, is_error?}`: the result of one `calls`, by its `id`; it continues the same turn. A result nobody waits for is `error {error_code: "call_unknown", kind: "tool_result"}` |

`turn_id` and `question_id` come back in the type they came in (an int stays an int).

A world call does not end the turn. The model calls as many world tools as it wants; each one
leaves as `call`, waits for its `call_result` and goes back into the same turn. The turn ends
when the model answers without a call (after at most one follow-up, below). The menu of a turn is its whole menu: a call of a tool
the open turn did not offer (a world tool the `obs` left out, `phone_message` in a phone or
interview turn) leaves nothing and is answered as an error result
`{id, content, is_error: true, error_code: "tool_not_offered"}`, the code in `content` too. The world answers a `query` at once and checks and
queues an `act`/`task` (at most one `task` per turn), carrying it out when the `intent`
arrives.

A world turn whose model answers in text alone, without a single world call, has moved nothing
in the world. The agent then asks once more: the world turn names its menu and `phone_message`
as `hop.required_tools` with `required_mode: "any"` (one call of any of them is enough) and the
words of the follow-up as `required_ask`, and the door of `simple-agent@2` sends one follow-up
turn in the same session with the same menu, asking the model to choose one action from the
world's tools. In mode `any` the follow-up's first model call must call a tool
(`tool_choice: "required"`, on the Anthropic wire `{"type": "any"}`); the calls after a result
are free again, and every other turn (the world turn itself, phone, `contact`, interview) requires
nothing. Its calls leave under the world's `turn_id`; the door's one answer ends the world turn,
and its text joins both texts. There is no second follow-up. A `contact` turn, a
turn whose world offered no tool of its own (`phone_message` alone does not count), a turn that hit
the agent's loop cap (`partial`: its budget is spent), a phone and an interview turn get none. A
text message counts as an action; a call of an extra tool does not. The door counts the calls the
model made, not their results: an empty `phone_message`, which the world refuses (`empty message`,
nothing leaves the phone), counts too and costs no follow-up (GH #1315).

The figure never cuts a turn short: `deadline_ms` is the world's. A lost `call_result` ends
the turn through the agent's own guard (`round_idle_ms`), so an instance sets the agent's
`round_idle_ms` (door and `collector/assemble`) at least as long as the longest deadline its world gives.

An `obs` without an integer `day` runs no turn: `error {error_code: "obs_without_day"}` and
`intent {turn_id, never: true}`. A `turn_id` already taken runs once; a copy is answered with
`error {error_code: "duplicate_turn"}` alone.

## Extra tools

`world.extra_tools` (default `[]`) is a list of client tools (provider schemas) the figure
offers on every turn -- world, phone, interview, follow-up -- beside the world's menu. A call of
one leaves the figure on its rim as `calls` and waits for its `tool_result` on the rim, which
goes back into the round the call was asked in; the turn goes on. An extra tool whose name the
turn already offers (a world tool, `phone_message`) stays the world's. An extra call moves
nothing in the world: it neither ends the turn nor spares the follow-up. Without an entry the
turns are the same, byte for byte. The colony draws `calls` to whoever serves the tool and its
answer back to the figure's `tool_result`.

## The handset (V-H)

The phone is a human channel. A message of the figure leaves the figure as

```json
{"header": {"route": "turn", "chat_id": "<prefix>:<world_id>:<figure_id>"},
 "messages": [{"origin": "user", "type": "text", "text": "..."}]}
```

the form a [`door`](../door/) emits, so the colony on the other side cannot tell it from a
person typing. It carries `hop.happened_at`, the `at` of the world's last `obs`, when the
world gave one (absent otherwise, never empty). An `answer` coming back with `context.channel` equal to that `chat_id` becomes a
phone turn (channel `phone:<world_id>`, session `phone-<day>`, no client tools, opening
"Message from <contact>: ..."). Its final text is the reply and leaves the same way. A phone turn
may stay silent, and its opening says so: an empty final text, or an end without text, sends
nothing.

Replies are capped: at most `phone_replies_max` answers (default 4) open a phone turn per world
day. An answer over it opens no turn and costs no model call: it leaves on the rim as
`phone_capped {day, channel, text, replies, max}` for the world's journal, and the figure's
next world turn lists it as a message that reached the phone unanswered, so it enters the
figure's memory with that turn. Two agents that text each other stop there for the day.

One colony turn reaches the handset once. A colony may send a second answer of the same turn
(an advice or a delegation that came back in time), under `hop.round_id` `<round>~<ms>~<hash>`;
the handset hands the base `<round>` to the world. An answer whose base round the world has
already taken (the last 64) opens no turn, is not capped and spends no reply: with the same text
it is dropped, with another text it is a correction and waits unread for the next world turn, as
one on a phone would. An answer without `hop.round_id` is taken as before.

An `answer` on another channel is refused with
`error {error_code: "foreign_channel"}`.

Texting is asynchronous: the tool `phone_message {text}` is offered on world turns of a
`human-sim` whose phone is enabled; it answers `{status: "sent"}` at once and the turn goes on.
A call without text is answered as an error result (`empty message`).

The switch `phone.speaker`:

| value | prefix | hop |
|---|---|---|
| `member` (default) | `hp` | none extra: the figure speaks as the member of the colony |
| `game_agent` | `game` | `speaker: "game_agent"` |

Nothing else changes. The edge pair at the colony's human door is the instance's (one
manifest: `add_nodes` plus the pair), never the template's.

## One turn at a time

The agent has one caller. A turn that arrives while another is open (a phone message during a
world turn, an interview, the night) waits in the adapter's queue and goes in when the open
turn's `answer` arrives. A `call` of the agent that finds no open world turn of its id is
answered as an error result. The queue is state of a resident cell: RAM is its cache, and a
replaced child starts with an empty queue (see Limits).

## Roles

`world.role` is required (`operator_set`): `human-sim` or `npc`. Each role gives the agent its
frame once, at boot, as the pack leaf `instructions.role`, beside the agent's own
`instructions.frame` (the identity leaf belongs to affinity's pack):

- `human-sim`: an AI living the life of the person its identity describes, in a world shared
  with others, in the first person.
- `npc`: an AI playing the role its identity describes in a game world: it reacts to the
  players, stays in its role and follows the state of the game as the world tells it.

Both frames say the same two things: asked directly whether it is an AI, the figure says
truthfully that it is; and it may decline anything, in its own words. Any other value of `role`
refuses every turn with `error {error_code: "role_unknown"}`. The world's frame reaches the
agent's curator on the agent's own lane `in_pack`; its receipt goes back to the world, never out
of the figure.

## Identity

The figure's identity is its own entry `entity:<agent_id>` in the colony's affinity: the AIeOS
document of `simple-agent/enrol` (`identity`), written by the agent's enrolment on a commit
receipt that concerns it (a boot, a manifest), through the figure's rim (`affinity_propose`
with `context.sa_enrol`, back on `in_propose_ack`). The pack of that entry reaches the agent on
`in_pack`. Keep `simple-agent/door` and `simple-agent/enrol` on the same `agent_id`, and keep it
the figure's id.

The figure reads no one else's entry. Its turns name no counterpart, so the agent asks the
colony's affinity for no brief, and the enrolment discloses one field, the figure's own AIeOS,
to the figure itself (the default of `enrol/disclose`; the figure's ref sets nothing else). A
colony keeps it that way: no disclosure of the member's entry to the figure.

## Tools

The agent's own tool families are all off: the figure's ref names `tool_gate.tools_enabled` as
`[]`. A figure acts through its world's tools and, where an instance gives them, its extra
tools; the world's tools stay client tools of each turn and never become the agent's.

## The colony's lanes

The figure is sealed. It carries every contract lane of `simple-agent@2` on its own rim, one
level deeper (the agent's README, "Embedding one level deeper"): a lane that docks inside the
agent docks at the same cells under `./simple-agent`, and what is required of the agent is
required of the figure. The colony's manifest draws them; the figure draws none outward.

| lane | direction | `at` / inner target | required | what the colony draws |
|---|---|---|---|---|
| `credential_request` | out | `./simple-agent/brain`, `./simple-agent/strong`, `./simple-agent/curator/summarizer`, `./simple-agent/memory-hive/closer`, `./simple-agent/memory-hive/dialectic`, `./simple-agent/memory-hive/dreamer`, `./simple-agent/memory-hive/judge`, `./simple-agent/memory-hive/embed`, `./simple-agent/tools/web_search` | -- | one edge per consumer to the colony's access |
| `in_sealed` | in | the same nine | yes | one edge per consumer from the colony's access, the grant rows, the grant ids in `override_params` |
| `in_pack` | in | `./simple-agent/curator` | yes | the colony affinity's push for `agent:<agent_id>`, `pack_sub`/`pack_hash` promoted |
| `pack_ack` | out | the rim | -- | back to the affinity's `in_pack_ack` |
| `in_briefing` | in | `./simple-agent/collector` | yes | the affinity's brief answer (a figure asks none) |
| `brief` | out | the rim | -- | to the affinity's `in_brief` |
| `in_model` | in | the rim, on to the agent | yes | the colony llm-registry's pushes |
| `model_refused` | out | the rim | -- | back to the llm-registry |
| `affinity_propose` | out | the rim | -- | to the affinity's `in_propose`, `actor` from `hop.agent_id`, the subscriber of the enrolment's `subscribe` steps |
| `in_propose_ack` | in | the rim, on to the agent | -- | the affinity's `ack` and `error` back |
| `in_note` | in | `./simple-agent/curator` | -- | notes for the window |
| `in_conversations` / `conversations` | in / out | the rim, on to the agent | -- | the conversations a reader may see |
| `mutation_committed` | in | the rim, on to the agent | -- | the colony's commit receipts (the menu and the enrolment) |

The grants are the colony's: the figure names none, and an instance hands one grant per
consumer in at birth, as for the agent.

## Instance params

Set with `override_params` when the figure is grown. A key at the top level replaces the
shipped value whole (no deep merge), so `phone` is given as a whole object.

| cell path | param | meaning |
|---|---|---|
| `world` | `figure_id` | the figure's id: every turn runs in the audience `member:<figure_id>`; part of the phone's `chat_id` |
| `world` | `world_id` | the world's id: channels `world:<world_id>`, `phone:<world_id>`, `interview:<world_id>` |
| `world` | `role` | `human-sim` or `npc` (required) |
| `world` | `phone` | `{enabled, speaker, contact}`: `enabled` offers `phone_message` (human-sim only), `speaker` is the switch above, `contact` the name a phone turn names its sender with |
| `world` | `phone_replies_max` | `4`: answers on the handset that open a phone turn per world day; over it `phone_capped` |
| `world` | `extra_tools` | `[]`: client tools offered on every turn beside the world's menu (above) |
| `simple-agent/<cell>` | every `simple-agent@2` param | addressed by the cell's path inside the agent, e.g. `simple-agent/brain` (`model`, `reasoning_effort`, `x_title`, `http_referer`), `simple-agent/door` (`agent_id`, `start_memories`), `simple-agent/enrol` (`agent_id`, `identity`), every model cell and `simple-agent/tools/web_search` (`credential_grant_id`, at birth), `simple-agent/door` and `simple-agent/collector/assemble` (`round_idle_ms`, both the same value) |

The app attribution is the agent's: every model cell of `simple-agent` and its embedder
(`simple-agent/memory-hive/embed`, since GH #1315) carry `x_title` and `http_referer`, and an instance names its origin there (`<product> <colony>`).

## The rim is marked

Lanes like `night` and `interview` go both ways under one name, and a hive picks its edges by
`from` alone. Everything the adapters send out carries `hop.figure_rim = "out"`, and every door
edge asks for `!has(hop.figure_rim)`, so a message on its way out is never taken back in. Every
turn carries `context.figure_turn`, `context.figure_kind` and `context.figure_day`, so a world
reads its runs out of the message log; the exits delete them.

## Limits

- RAM at most 0.5 GB per figure, the agent included.
- Model cost at most 0.03 $ per figure-day without contact (the turns of a day plus the
  night's close and dream).
- The queue, the open turn, the call book, the replies counted today, the answer rounds already
  taken and the capped or corrected answers not yet shown live in the resident cell's RAM. A
  replaced child forgets them (the count starts again, a round seen before is taken anew): a `call_result` it no longer knows is dropped, a call of the agent finds no open
  turn and gets an error result, and a final answer is matched by its channel instead.

## Pins

`crates/meclaw-cells/tests/figure/figure_template.rs`: three children and the ref to
`simple-agent@2`, the world and phone lanes, the contract lanes of the agent one level deeper,
no grant and no access, no own tool family, no counterpart on any turn, the extra tools, no old
names and no product names, the required role and its two frames, and the adapters' answers to
each lane. `crates/meclaw-cells/tests/figure/figure_turns.rs`: the turns in a colony with the
colony's access and affinity, stub models only -- the identity as the figure's own entry, no
brief, the world's tools as client tools, the follow-up, the extra tools on the rim, the
handset.
