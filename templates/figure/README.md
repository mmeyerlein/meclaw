# `figure@1.0.1`

A character in a shared world, built as an onion: inside, an unchanged
[`simple-agent`](../simple-agent/) (one model behind a curator, a memory, its own tools, the
turn door); around it, two adapters. `world` speaks the world's lanes, `phone` speaks to a
colony exactly the way a human channel does. The world stands beside the figure, never inside
it: it renders the situation and hands its tools to each turn as client tools, and the figure
calls them like any other tool until the model stops.

One instance per character, in a world colony or in a player's own colony alike.

## Cells

| path | type | what it does |
|---|---|---|
| `world` | `code`, resident | the world adapter and the ONLY sender into the agent's turn door: every turn (world, phone, interview) runs through it, one at a time |
| `phone` | `code`, warm | the handset: turns a message of the figure into a human-channel `turn`, and an `answer` on that channel into a phone turn |
| `simple-agent` | ref [`simple-agent`](../simple-agent/) | the agent itself, referenced, never copied |

No model cell of its own, no memory of its own, no grants of its own: all of that is the
agent's.

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
in the world. The figure then asks once more: a follow-up turn in the same session with the same
menu (`turn_id` `<turn>:act` at the agent's door), asking the model to choose one action from the
world's tools. The follow-up carries `tool_scope: {require: true}`, so its first model call must
call a tool (`tool_choice: "required"`, on the Anthropic wire `{"type": "any"}`); the calls after a
result are free again, and every other turn (the world turn itself, phone, `contact`, interview)
requires nothing. Its calls leave under the world's `turn_id`; whatever it ends with ends the world
turn, and the `intent` text joins both texts. There is no second follow-up. A `contact` turn, a
turn whose world offered no tool of its own (`phone_message` alone does not count), a turn that hit
the agent's loop cap (`partial`: its budget is spent), a phone and an interview turn get none. A
text message counts as an action.

The figure never cuts a turn short: `deadline_ms` is the world's. A lost `call_result` ends
the turn through the agent's own guard (`round_idle_ms`), so an instance sets the agent's
`round_idle_ms` (door and `collector/assemble`) at least as long as the longest deadline its world gives.

An `obs` without an integer `day` runs no turn: `error {error_code: "obs_without_day"}` and
`intent {turn_id, never: true}`. A `turn_id` already taken runs once; a copy is answered with
`error {error_code: "duplicate_turn"}` alone.

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
refuses every turn with `error {error_code: "role_unknown"}`. The identity itself is not the
figure's: it is the agent's AIeOS document (`simple-agent` params `agent_id`, `aieos`,
`start_memories`), written once into the agent's own affinity.

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
| `simple-agent/<cell>` | every `simple-agent` param | addressed by the cell's path inside the agent, e.g. `simple-agent/brain` (`model`, `reasoning_effort`, `x_title`, `http_referer`), `simple-agent/door` (`agent_id`, `aieos`, `start_memories`), `simple-agent/tools/file` and `simple-agent/tools/edit` (`base_path`, both the same), `simple-agent/door` and `simple-agent/collector/assemble` (`round_idle_ms`, both the same value) |

The app attribution is the agent's: every model cell of `simple-agent` carries `x_title` and
`http_referer`, and an instance names its origin there (`<product> <colony>`).

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

`crates/meclaw-cells/tests/figure/figure_template.rs`: three children and the ref, the world
and phone lanes, no old names and no product names, the required role and its two frames, and
the adapters' answers to each lane.
