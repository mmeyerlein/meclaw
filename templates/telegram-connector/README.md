# `telegram-connector@2.0.1`

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
the library shipped before this one sat inside a complete bot -- each with its
own model, its own persona and a seal with no door, so a colony that had
assembled its own agent had no way to put a chat surface in front of it and
started with a hand-written proxy instead (GH #246).

**One field differs from the plain Telegram proxy:**
`params.emit_to` points one level up (`..`) instead of at a persona cell, because
here every emission is routed by the edges of whatever holds the connector.
`emit_to` is only the no-route address; the substrate applies the edge table to a
source emission too.

## The cell

| path | type | from |
|---|---|---|
| the template root itself | `proxy` | the plain Telegram proxy, but for `emit_to` |

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
              "set_context": {"channel_node": "'telegram-connector'",
                              "channel": "has(hop.chat_id) ? hop.chat_id : ''",
                              "chat_id": "has(hop.chat_id) ? hop.chat_id : ''",
                              "user_id": "has(hop.user_id) ? hop.user_id : ''"}}},
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
downstream template reads it by (`firewall` rate-limits per channel,
`talky` mints one session per channel) — **a chat is one conversation partner
and it gets one of each**, which is why the chat id and not the node name stands
there ([#522](https://github.com/mmeyerlein/meclaw/issues/522)).

`channel_node` is the other half of the same split, and it is the NODE this cell
stands at. An edge target is a static path in this substrate, so the way back
has to say which child of a container it is for; a container holding two
connectors would route every answer of both by the same word if that word were
the chat. Where the connector stands in a member's `./channels`, the node name is
the channel's name and the level publishes the rule
(`templates/member/README.md` § *The two channel keys*). Every promotion is
written `has(...) ? ... : ''`: a modifier that fails to evaluate skips the whole
edge, and this cell's own failure emissions carry no chat at all — they ride the
second edge, which promotes nothing.

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
{"name": "telegram-connector-2", "template": "telegram-connector@2.0.1",
 "override_params": {"bot_token": "${TELEGRAM_BOT_TOKEN_2}"}}
```

**One `getUpdates` consumer per token.** A second poller on the same token gets
`409 Conflict` and the two steal each other's updates. Until GH #468 that error
was an ordinary transient on DEBUG, so the only symptom was a bot answering every
other message; it is now a class of its own and leaves a `warn` line carrying
`error_code = "conflict_other_poller"`. The recovery is unchanged -- the lane
keeps polling with the transient backoff, because the other consumer may stop and
a switchover is exactly the case where it does.

It is a **log line, not an emission**. The poll lane answers no message, so a
receipt would have to be a source emission carrying `hop.error_code` -- a fifth
failure code every level holding a connector would owe a drain for, repeated on
every backoff tick, for a condition only an operator can fix. The four codes in
the table above are still the whole set.

Two consequences, and both are load-bearing:

- One instance per token. Two colonies, or two connectors, on one token is a
  misconfiguration that diagnoses badly.
- **A connector must never live inside something that gets instantiated more than
  once, or woken more than once.** That is why it does not live inside a talky
  generation: waking an old generation to ask it something would wake its poller
  too (ADR-0002, finding B1). The token belongs to the channel, not to the
  generation -- which is what the `channels` level exists to keep apart (GH
  #303).

## Born asleep, armed by a mutation

A connector cannot be grown quietly. The moment its cell has a task it opens the
long poll, and the token permits exactly one consumer -- so growing a channel
into a colony whose predecessor is still running takes the upstream away from the
one that is answering. The old workaround was to edit `config.json` in the target
tree afterwards, which is the one build form this project does not allow: the
file is a bootstrap imprint written once, at instantiation, and nothing reads an
edit of it until the next spawn.

The canonical form is two mutations. Both go through the door, both leave a
receipt in the `mutation_log`, and neither touches a file by hand.

**1. Grow it asleep.** `birth: "inactive"` (GH #437) registers the node, persists
it inactive and builds **no task**, so nothing polls. The wiring is laid down in
the same breath -- a node born inactive is not reached by the connectivity
recompute of its own birth mutation, so the edges are already there and still
dark. And it STAYS dark: since [#491](https://github.com/mmeyerlein/meclaw/issues/491)
the declaration is durable rather than a starting value, so no later mutation
elsewhere in the tree can wake it by recomputing over it -- only one that names
this node itself:

```json
{
  "scope": "/",
  "diff": {
    "add_nodes": [
      {
        "name": "telegram",
        "template": "telegram-connector@2.0.1",
        "birth": "inactive",
        "override_params": {
          "bot_token": "parked",
          "base_url": "http://127.0.0.1:9"
        }
      }
    ],
    "add_edges": [
      {"from": "./telegram", "to": "./sink",
       "condition": "!has(hop.error_code)",
       "modifier": {"set_context": {"chat_id": "has(hop.chat_id) ? hop.chat_id : ''",
                                    "user_id": "has(hop.user_id) ? hop.user_id : ''"}}},
      {"from": "./telegram", "to": "./sink", "condition": "has(hop.error_code)"},
      {"from": "./sink", "to": "./telegram"}
    ]
  }
}
```

All three edges belong in this mutation, and the third is not optional: the
connector's contract requires `context.chat_id`, and a wiring that never gives it
a way back is refused as `edge_schema` before anything is staged. Parking a
channel means parking a **complete** one.

The two placeholders are not what keeps it quiet -- `birth` does that on its own.
They are the seatbelt for the moment somebody wakes the node by accident, and
each answers a different refusal:

- `bot_token` must be **non-empty** or the instantiation is refused outright
  (`invalid_params`, GH #270): an empty token used to poll `.../bot/getUpdates`
  in a 404 loop. A literal placeholder is deliberate here, and it is the one
  place in this template where the token is not a `${VAR}`: a parked node must
  not hold the real credential, because holding it is what arming means.
- `base_url` points at a **closed port**. Port 9 (discard) refuses the connection
  immediately, so a woken placeholder backs off against nothing instead of
  reaching Telegram with a token that is not a token.

**2. Arm it with a `swap_nodes`.** The parked node cannot be armed in place:
`bot_token` is immutable on the runtime params surface (a params update naming it
is a loud `Immutable` reject), and `config.json` is never rewritten. So arming
swings the edges onto a freshly instantiated implementation that carries the real
credential -- the graph swap `swap_nodes` was re-dedicated for:

```json
{
  "scope": "/",
  "diff": {
    "swap_nodes": [
      {
        "match": {"name": "telegram"},
        "with": {
          "name": "telegram-live",
          "template": "telegram-connector@2.0.1",
          "params": {"bot_token": "${TELEGRAM_BOT_TOKEN}"}
        }
      }
    ]
  }
}
```

Three things to know about that swap:

- **The address changes.** `swap_nodes[].with` instantiates a fresh cell, and its
  target path must be free -- so the armed node cannot reuse the parked node's
  name. That costs nothing as long as the connector is reached by **edges**,
  which swing with the swap; anything that addressed it by a hard-coded path has
  to be re-pointed.
- **`with` has no `birth`.** A successor born inactive would leave the swapped
  edges pointing at nothing, so the armed node always starts polling at once.
  That is the whole point of the operation.
- **The parked node survives, disconnected.** No-delete: its directory,
  `cell_id` and `cell.db` stay, and swinging the edges back is a mutation like
  any other.

**The token stays a `${VAR}` everywhere it is real.** A mutation body naming a
value ships a secret into the `mutation_log`; the substitution happens once, at
instantiation, out of the colony's `.env`.

## The switchover order

Two colonies on one token is the failure this template diagnoses worst, so the
order is not a preference:

1. **Stop the old poller first.** Either the whole old colony, or a
   `remove_edges` that disconnects its connector -- an edge withdrawal ends the
   task, the registry row stays.
2. **Wait until it is gone.** Not a pause for form's sake: the old process has to
   have closed its long poll before the new one opens. A connector that is armed
   into the window gets `409 Conflict`, and both sides then lose updates to each
   other. The `warn` line named above is what that window looks like from the
   inside.
3. **Then arm the new one**, with the `swap_nodes` above.

**The update cursor needs no migration.** `getUpdates` is offset-based and the
offset lives in the new cell's own `cell.db`, starting at 0 -- Telegram then
replays whatever it still holds and the cursor catches up on the first poll.
There is nothing to copy out of the old cell, which is what makes the swap a
topology operation rather than a data one.

**The credential is an operator step, not a builder one.** Where the token lives
in a vault instead of `.env`, the value is put in by hand
(`meclaw --vault-add`, on stdin, never in a mutation body) and the grant rows
travel in the seed, in place **before** the first spawn of the cell that will
spend them. A builder only ever emits a `credential_grant_id`; it never emits a
credential.

## While a turn is running

The connector types. On every inbound message it calls Telegram's
`sendChatAction` with `action=typing`, and it repeats that call every 4 seconds
for at most 60 seconds, until the answer for that chat goes out. The chat shows
"typing…" and **nothing is written into it** -- which is the whole reason this is
`sendChatAction` and not a placeholder message: a connector that posts "still
working" into the conversation has changed the transcript the agent behind it
will read back on the next turn.

Both numbers are forced. Telegram drops the status after roughly five seconds,
so a single call covers only the first moment of a turn and the refresh needs a
full second of margin under that decay. The ceiling exists because nothing tells
a connector that a turn was abandoned somewhere in the topology -- without it, a
turn that dies leaves the chat typing forever.

It is behaviour, not a setting: there is no params key for it, and there is
nothing to wire. One repeater per chat at most -- a second message in the same
chat replaces the first one's repeater instead of stacking a second one on it --
and the answer cancels it, so a chat that got its answer stops typing at once
(GH #515).

**A failed chat action never costs the turn its answer.** It is logged and the
next refresh tries again; the connector's four `error_code`s in the table above
are still the whole set.

## What it is not

- **Not a screen.** It has no allowlist and no rate limit; anybody who finds the
  bot reaches your topology. Put [`firewall`](../firewall/) behind it --
  shared across channels, so an attacker touching three of them is one pattern
  and not three thirds.
- **Not a bot.** This is the wire, and the agent behind it answers. A complete
  bot is this connector plus something that produces answers -- see the
  [`talky`](../talky/) template.
- **Not a session, not a memory, not a history.** It reads `message.text` and
  nothing else -- a join or a leave carries no `text` and falls silently on the
  floor, so a topology that needs to know the participant set changed cannot
  learn it here yet (ADR-0002, O2).
- **Not a level.** It normalises nothing on its own behalf any more; the level
  that holds it names the lanes (GH #303).
