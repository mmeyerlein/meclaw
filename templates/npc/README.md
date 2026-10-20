# `npc@1.0.1`

A non-player character for a shared world: one identity seeded at instantiation, ONE `llm`
brain behind a [`curator`](../curator/), a [`memory-hive`](../memory-hive/), the world's tools
as the menu of each move, and a handset. A slimmed-down agent in hive form: sealed
(`params.ports: []`), its own [`access`](../access/) with sealed vault grants, the `in_model`
door. The tool round inside is the one of [`cogny`](../cogny/) (collector -> curator -> brain
-> splitter -> dispatcher), copied edge for edge.

One instance per character, in a world colony or in a player's own colony alike. The world
owns the situation, the deadlines and the check of what was said; the character owns its
window, its memory and its decisions.

## Cells

| path | type | from |
|---|---|---|
| `world` | `code`, resident | the adapter: the only Python of this template |
| `collector` | ref | [`collector`](../collector/) |
| `curator` | ref | [`curator`](../curator/), its summarizer on its own grant |
| `brain` | `llm` | the one thinking cell, shaped like `cogny/brain` but a fast speaking model: born on the catalogue row of `talky`'s voice, `max_tokens` 16384 (a day plan as one tool call, the floor of GH #1037), its timeouts derived from both |
| `splitter` | `code` | a copy of `cogny/splitter` (sidecar sections `window`, `gap`, `memory`) |
| `dispatcher` | ref | [`dispatcher`](../dispatcher/) |
| `memory-hive` | ref | [`memory-hive`](../memory-hive/) |
| `access` | ref | [`access`](../access/), seeded with the grants of the seven model consumers |

No session keeper, no member, no apps, no companion, no second thinking cell.

## Lanes

Every lane carries the body slot of its own name.

| lane | direction | body |
|---|---|---|
| `obs` | in | `{turn_id, kind: plan\|contact\|event, text, tools[], deadline_ms?, must_say_first?, day}`: `tools[]` is the world's full menu; the template sets no cap. Refused with `error` code `obs_without_day` or `duplicate_turn` (below) |
| `intent` | out | `{turn_id, intents: [{tool, args, says?}]}`; text beside a call is that intent's `says`; text alone is `{turn_id, intents: [{says}]}` (speech, no tool); `{turn_id, never: true}` only for a move with neither text nor call, or a failed one. A move whose only call is `phone_call` sends that message on `hp` and the intent `{turn_id, intents: [{says}]}` with text beside it, `{turn_id, never: true}` without |
| `night` | in / out | in `{day}`, the day that ends: it closes the session `day-<day>`, and the next `obs` carries the next day; out `{day, ok, added, closed}` after the memory's close report |
| `digest` | in | `{day, text}`: one episode, no model call |
| `hp` | in / out | in: the handset, `{text, at, from}`, one round on channel `hp`; out: a message the character sends with `phone_call`, `{text, to, at, from}` with `to` from the call. A second delivery of the same message is answered with `error {error_code: "duplicate_turn", kind: "hp"}` alone |
| `hp_reply` | in / out | in: an answer to a `phone_call`; out: the character's answer to an `hp` message, `{text, to, at, from}` with `to` the sender of the `hp`: the text beside a call that ends the round (any but `memory_recall` and `history_*`), or the round's final text. A tool the handset does not offer, called without text, is answered with `error {error_code: "tool_not_offered", kind: "hp"}`. Text beside `memory_recall` or `history_*` is an interim and never sent |
| `identity` | in | `{facts[], since}`: replaces the curator's pack leaf `persona.current` and is written as an episode |
| `mandate` | in | `{action: start\|stop, until?, frame?, note?}`: the pack leaf `instructions.mandate` until a `stop` |
| `interview` | in / out | in `{question_id, question}`; out `{question_id, answer}`: recall at tier 1, one call, no tools, nothing written; a failed call answers `{question_id, answer: null, error}` |
| `in_model` | in | a model package of the llm-registry for the addressed model cell; without `hop.subscriber` it goes to `./brain` |
| `model_refused` | out | a package a model cell refused |
| `error` | out | a failed round `{error_code, turn_id, kind}`, or a failure of `./access`; never silent |

`turn_id` comes back unchanged, in the type it came in (an int stays an int; `question_id`
alike). An `obs` without an integer `day` runs no move: it is answered with
`error {error_code: "obs_without_day", turn_id}` and `intent {turn_id, never: true}`. A
`turn_id` the character already took runs once: a second copy is answered with
`error {error_code: "duplicate_turn", turn_id}` alone, and the first move runs on. The
start memories reach the memory once, on the first day the character opens, whatever day that
is. The curator rebuilds the window itself (at the model's `input_soft`, over `input_hard`, or when keeping the cached rest costs more than a rebuild): no world lane is needed for it. A handset round offers `phone_call` (with the memory's own tools), never the world's menu
of the last move; the next `obs` sets the world's menu again. The adapter stamps `context.npc_turn`, `context.npc_kind`,
`context.npc_day` and `context.npc_origin` on every turn, so a world reads its runs out of the
message log.

## Instance params

Set with `override_params` when the character is grown.

| cell | param | meaning |
|---|---|---|
| `world` | `npc_id` | the character's id: every round runs in the audience `member:<npc_id>` |
| `world` | `identity` | the seed: `name`, `core`, `origin`, `life_goal`, `facts[]`, `voice`, `start_memories[{what, with, days_ago}]` |
| `world` | `world_start` | RFC 3339 instant of day 1; the start memories are dated `days_ago` before it |
| `world` | `phone` | `"1"` offers `phone_call` on every move; `""` or `"0"` leaves the handset receive-only |
| `brain` | `model` | `${ctx.model}`, resolved at instantiation: a fast speaking model; another row than the shipped one derives `external_timeout_ms` and `message_timeout` again |
| `brain` | `reasoning_effort` | unset (null): no thinking; an instance may set `low`, `medium` or `high` |
| `brain`, `curator/summarizer`, `memory-hive/{closer,dialectic,dreamer,judge}` | `x_title`, `http_referer` | the app attribution; the default is the environment's |

The provider key is one vault deposit, `meclaw --vault-add cred:openrouter`; the seven grants
that name it ship with the template.

## Two things that are not obvious

**A world call is a handoff.** The dispatcher's expectation set runs through `./world`
(`./dispatcher -> ./world` on `calls`), which names every call that is not the round's own
(`memory_recall`, `history_*`) in `hop.async_calls` and `hop.handoff_calls` before it hands
the set on to the collector. The collector answers a handoff with a plain result and opens no
expectation, so a move costs one model call, the window stays well-formed, and the move leaves
once, as one `intent`. The curator keeps a tool call in its window only beside its result, so
the adapter writes the move into the head of the character's next turn ("What you did last").

**The rim is marked.** Lanes like `night`, `hp` and `interview` go both ways under one name,
and a hive picks its edges by `from` alone. Everything the adapter sends out carries
`hop.npc_rim = "out"`, and every door edge asks for `!has(hop.npc_rim)`, so a message on its
way out is never taken back in.

## The frame

Every character is told the same frame before its identity: the leaf `identity.frame` stands
first in the brain's system prompt, ahead of `identity.npc`. It says the character is an AI that
lives this person in a shared world, in the first person, that says truthfully that it is an AI
when asked directly, and that may decline. It never claims to be a human. `must_say_first` stands verbatim at the
head of its move; checking it is the world's.

## Pins

`crates/meclaw-cells/tests/npc/npc_template.rs`: the lanes and the seal, the memory's
drains, the identity and mandate leaves, exactly one thinking `llm`, the attribution and
grant of every model cell, a fast speaking brain, the start memories of a character born
late, an `obs` without its day refused loudly, and in a colony with stub models one intent
per move after one brain call (an int turn id comes back an int), the frame before the
identity at the brain, a night that closes with a model call and runs the dream, and a
handset round that sees the world's move but not its tools.
