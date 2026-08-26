# `telegram-connector@2.0.0`

A Telegram chat as one cell. One `proxy`, one credential, one wire in and one
wire out. No persona, no llm cell, no answer of its own -- it carries turns
between a chat and whatever you put behind it.

**`2.0.0` is a removal, and that is why the first digit moved.** Up to and
including `1.0.0` this template was a sealed hive around the `proxy` cell:
`params.ports` was empty, the address was the hive, and what a caller wanted rode
on `hop.route` -- `in_reply` in, `turn` and `error` out, rewritten by three edges
the hive owned. The hive grouped exactly one cell, which is not a level (ADR-0002,
addendum of 2026-08-20), so it is gone and the cell moved up. A caller that wired
`./telegram-connector` plus `hop.route == 'in_reply'` must now wire the cell
directly and read `hop.error_code` to tell an inbound turn from a connector
failure. Taking a documented address away is neither of the two cases
`docs/development-rules.md` § 4 covers -- a repair moves the third digit, an
addition the second -- so it is the first (GH #303).

**This is the building block the demo bots were welded around.** Every proxy cell
the library shipped before this one sat inside a complete bot -- `bot-basic`,
`slack-agent`, `egon`, `research-assistant`, `daily-digest` -- each with its own
model, its own persona and a seal with no door, so a colony that had assembled
its own agent had no way to put a chat surface in front of it and started with a
hand-written proxy instead (GH #246).

**The cell is taken verbatim from `bot-basic@2.0.0`.** One field differs:
`params.emit_to` points one level up (`..`) instead of at a persona cell, because
here every emission is routed by the edges of whatever holds the connector.
`emit_to` is only the no-route address; the substrate applies the edge table to a
source emission too.

## The cell

| path | type | from |
|---|---|---|
| the template root itself | `proxy` | `bot-basic@2.0.0`, verbatim but for `emit_to` |

Nothing sits below it. `./telegram-connector` is the node, not a scope with a
door -- there is no `hive_port_boundary` to trip over and no lane name to hit.

## What travels

| direction | what travels |
|---|---|
| in | the finished assistant turn. `context.chat_id` picks the chat |
| out, without `hop.error_code` | one inbound chat message as a user-origin turn. `hop` carries `chat_id`, `user_id`, `message_id`, `platform` |
| out, with `hop.error_code` | the connector's own failure: `missing_chat_id`, `missing_assistant_turn`, `send_failed`, `invalid_body` |

Both outbound shapes leave on the same wire. **Sorting them is the caller's job
now**, and it is the same two conditions the dissolved hive carried, moved out to
where the edges are written: `!has(hop.error_code)` is the turn,
`has(hop.error_code)` is the failure.

**The level that holds this connector owes the `error` drain.** The hive used to
state that as `params.required_drains` (`in_reply` paired with `error`), and a
single cell cannot declare a hive drain pairing -- so the obligation travels in
prose until the level that replaces the hive declares it in the lane form:
`channels` (GH #303). Until then nothing refuses a topology that wires the
inbound edge and leaves the failures unwired, and the failure that follows is the
one a chat surface actually has: the answer that never arrived, invisible at
exactly the end where somebody is waiting for it.

## Wiring it

Three edges, all naming the cell, all in the mutation that instantiates it:

```json
{"from": "./telegram-connector", "to": "./firewall",
 "condition": "!has(hop.error_code)",
 "modifier": {"set_hop": {"route": "'in_turn'"},
              "set_context": {"channel": "hop.chat_id", "chat_id": "hop.chat_id",
                              "user_id": "hop.user_id"}}},
{"from": "./telegram-connector", "to": "./sink",
 "condition": "has(hop.error_code)"},
{"from": "./agent", "to": "./telegram-connector",
 "condition": "has(hop.route) && hop.route == 'answer'"}
```

The first two are the pair: whatever the level around the connector calls its
turn lane and its failure lane, these are the two conditions that separate them,
and the second one is the drain the paragraph above owes. The third one is the
way back -- no `set_hop` is needed, because the cell reads `messages[]` and
`context.chat_id` and nothing else.

**The promotion on the first edge is not decoration.** `hop` is single-hop: it
survives one delivery. `chat_id` has to be in `context` before the turn reaches
anything that will emit again, or the reply has no chat to go to and the proxy
answers `missing_chat_id`. `channel` is the same identifier under the name every
downstream template reads it by (`firewall@2.0.4` rate-limits per channel,
`talky@4.2.2` mints one session per channel).

**Numbers on the hop need `int()`.** The proxy delivers JSON integers, CEL
deserialises them as `uint`, and a bare `hop.user_id == 12345` is silently
**false** -- no error, no log line. Every numeric condition carries the cast.

## The credential

`params.bot_token` is `${TELEGRAM_BOT_TOKEN}`, substituted once, at
instantiation, out of the colony's `.env`. After that the `config.json` is a
bootstrap imprint and nothing rewrites it: rotating the token means editing
`.env` and restarting.

A second connector in the same colony needs its own variable. The template is one
cell, so `override_params` takes the flat form -- there is no path inside it to
address:

```json
{"name": "telegram-connector-2", "template": "telegram-connector@2.0.0",
 "override_params": {"bot_token": "${TELEGRAM_BOT_TOKEN_2}"}}
```

**One `getUpdates` consumer per token.** A second poller on the same token gets
`409 Conflict` and the two steal each other's updates -- the error is transient,
stays on DEBUG, and looks like a bot that answers every other message. Two
consequences, and both are load-bearing:

- One instance per token. Two colonies, or two connectors, on one token is a
  misconfiguration that diagnoses badly.
- **A connector must never live inside something that gets instantiated more than
  once, or woken more than once.** That is why it does not live inside a talky
  generation: waking an old generation to ask it something would wake its poller
  too (ADR-0002, finding B1). The token belongs to the channel, not to the
  generation -- which is what the `channels` level exists to keep apart (GH
  #303).

## What it is not

- **Not a screen.** It has no allowlist and no rate limit; anybody who finds the
  bot reaches your topology. Put [`firewall@2.0.0`](../firewall/) behind it --
  shared across channels, so an attacker touching three of them is one pattern
  and not three thirds.
- **Not a bot.** `bot-basic@2.0.0` is the other shape of the same cell: a whole
  demo bot that answers by itself, which is why nothing is wired to it. Run that
  one whole; build with this one.
- **Not a session, not a memory, not a history.** It reads `message.text` and
  nothing else -- a join or a leave carries no `text` and falls silently on the
  floor, so a topology that needs to know the participant set changed cannot
  learn it here yet (ADR-0002, O2).
- **Not a level.** It normalises nothing on its own behalf any more; the level
  that holds it names the lanes (GH #303).
