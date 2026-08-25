# `telegram-connector@1.0.0`

A Telegram chat as one address. One sealed hive, one `proxy` cell inside it, two lanes
out and one lane in. No persona, no llm cell, no answer of its own -- it carries turns
between a chat and whatever you put behind it.

**This is the building block the demo bots were welded around.** Every proxy cell the
library shipped before this one sat inside a complete bot -- `bot-basic`, `slack-agent`,
`egon`, `research-assistant`, `daily-digest` -- each with its own model, its own persona
and a seal with no door, so a colony that had assembled its own agent had no way to put a
chat surface in front of it and started with a hand-written proxy instead (GH #246).

**The `proxy` cell is taken verbatim from `bot-basic@2.0.0`.** One field
differs: `params.emit_to` points at the enclosing hive (`..`) instead of at a persona
cell, because here every emission is routed by the hive's own edges. `emit_to` is only
the no-route address; the substrate applies the edge table to a source emission too.

## Cells

| path | type | from |
|---|---|---|
| `proxy` | `proxy` | `bot-basic@2.0.0`, verbatim but for `emit_to` |

## Lanes

`params.ports` is empty. **The address is the hive**; what a caller wants rides on
`hop.route`, and the one door inside decides which cell that means. `./proxy` is not an
address -- an edge naming it is refused with `hive_port_boundary`.

| lane | direction | what travels |
|---|---|---|
| `in_reply` | in | the finished assistant turn. `context.chat_id` picks the chat |
| `turn` | out | one inbound chat message as a user-origin turn. `hop` carries `chat_id`, `user_id`, `message_id`, `platform` |
| `error` | out | the connector's own failure: `missing_chat_id`, `missing_assistant_turn`, `send_failed`, `invalid_body` |

`params.required_drains` pairs them: wire `in_reply` and you must wire `error` too. The
one failure a chat surface has is the answer that never arrived, and it is invisible at
exactly the end where somebody is waiting for it.

## Wiring it

Three edges, all at the hive path, all in the mutation that instantiates it:

```json
{"from": "./telegram-connector", "to": "./firewall",
 "condition": "has(hop.route) && hop.route == 'turn'",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"channel": "hop.chat_id", "chat_id": "hop.chat_id",
                              "user_id": "hop.user_id"}}},
{"from": "./agent", "to": "./telegram-connector",
 "condition": "has(hop.route) && hop.route == 'answer'",
 "modifier": {"set_hop": {"route": "'in_reply'"}}},
{"from": "./telegram-connector", "to": "./sink",
 "condition": "has(hop.route) && hop.route == 'error'"}
```

**The promotion on the first edge is not decoration.** `hop` is single-hop: it survives
one delivery. `chat_id` has to be in `context` before the turn reaches anything that will
emit again, or the reply has no chat to go to and the proxy answers `missing_chat_id`.
`channel` is the same identifier under the name every downstream template reads it by
(`firewall@2.0.4` rate-limits per channel, `talky@4.1.1` mints one session per channel).

**Numbers on the hop need `int()`.** The proxy delivers JSON integers, CEL deserialises
them as `uint`, and a bare `hop.user_id == 12345` is silently **false** -- no error, no
log line. Every numeric condition carries the cast.

## The credential

`params.bot_token` is `${TELEGRAM_BOT_TOKEN}`, substituted once, at instantiation, out of
the colony's `.env`. After that the `config.json` is a bootstrap imprint and nothing
rewrites it: rotating the token means editing `.env` and restarting.

A second connector in the same colony needs its own variable, addressed at the cell:

```json
{"name": "telegram-connector-2", "template": "telegram-connector@1.0.0",
 "override_params": {"proxy": {"bot_token": "${TELEGRAM_BOT_TOKEN_2}"}}}
```

**One `getUpdates` consumer per token.** A second poller on the same token gets
`409 Conflict` and the two steal each other's updates -- the error is transient, stays on
DEBUG, and looks like a bot that answers every other message. Two consequences, and both
are load-bearing:

- One instance per token. Two colonies, or two connectors, on one token is a
  misconfiguration that diagnoses badly.
- **A connector must never live inside something that gets instantiated more than
  once, or woken more than once.** That is why it does not live inside a talky
  generation: waking an old generation to ask it something would wake its poller too
  (ADR-0002, finding B1). The token belongs to the channel, not to the generation --
  see [`channel@1.0.0`](../channel/), which is this connector with a generation slot
  beside it.

## What it is not

- **Not a screen.** It has no allowlist and no rate limit; anybody who finds the bot
  reaches your topology. Put [`firewall@2.0.0`](../firewall/) behind it -- shared across
  channels, so an attacker touching three of them is one pattern and not three thirds.
- **Not a bot.** `bot-basic@2.0.0` is the other shape of the same cell:
  a whole demo bot that answers by itself, which is why it has no lane leaving its hive.
  Run that one whole; build with this one.
- **Not a session, not a memory, not a history.** It reads `message.text` and nothing
  else -- a join or a leave carries no `text` and falls silently on the floor, so a
  topology that needs to know the participant set changed cannot learn it here yet
  (ADR-0002, O2).
