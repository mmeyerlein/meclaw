# `chat-channel@1.0.0`

The channel `chat`, as one `code` cell. A sentence a person types on their own screen is
a turn of its own channel -- not a voice turn in disguise.

Every other way a person is reached already had a cell: `voice` carries the media,
`freeswitch` the telephone, `telegram-connector` a foreign protocol. Typing had none, so
a chat app pushed its input line straight at the member's firewall and had to invent
three things it does not own on the way: an identity, an assistant and a channel name, as
literals on an edge written by an installer. This cell is where those three stop being
literals.

## What it delivers

- **A `turn_id`, minted on acceptance.** The turn, its answer, and every window an app
  builds out of that answer carry the same id (`display-hive.md` § 8.2, § 8.3). It is
  random rather than ordered: this cell keeps no session -- the member's keeper does --
  and a counter here would restart at every respawn.
- **The member, from the channel.** Whoever types on a member's screen *is* the member
  (§ 8.7). The identity is the channel's fact: `params.user_id` by default, overridden by
  a `hop.user_id` the input line's event names itself.
- **A turn on the ordinary road.** `turn` leaves at this cell's own path and the member's
  edge stamps the context every turn is screened and routed by -- from there it is the
  firewall, the session, the talky, exactly as a spoken turn is.
- **Silence on the answer.** `in_answer` emits nothing. This channel has no loudspeaker
  (§ 8.3), and the answer the person reads reaches the chat app on the member's own
  `./assistants -> ./apps` edge, where every app that shows a conversation hears it.

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `code` | the id minting, the identity and the two doors. No state, no `cell.db`. |

Single-cell template: instantiate it as `<member>/channels/chat` and the instance IS the
cell. The source is `channel.py`; `scripts/chat_channel_sync.py` copies it into
`params.script_inline`, which is the form a shipped `code` template runs from.

## Ports and wiring

| lane | direction | what it is |
|---|---|---|
| `in_typed` | in | the event of a chat app's input line |
| `in_answer` | in | the talky's answer, absorbed |
| `turn` | out | the accepted sentence, with `turn_id`, `user_id`, `channel` and `happened_at` on the hop |
| `error` | out | `error_code: "empty_turn"` -- a line with no text is not a turn |

The two edges a channel costs, the pattern `voice` set:

```json
[
  { "from": "./channels/chat", "to": "./channels",
    "condition": "has(hop.route) && (hop.route == 'turn' || hop.route == 'error')",
    "modifier": {"set_context": {"channel_node": "'chat'", "channel": "'chat'",
                                 "assistant": "'<agent>'",
                                 "audience_set": "'[\"agent:<agent>\",\"member:<member>\"]'",
                                 "user_id": "has(hop.user_id) ? hop.user_id : ''",
                                 "turn_id": "has(hop.turn_id) ? hop.turn_id : ''"}} },
  { "from": "./channels", "to": "./channels/chat",
    "condition": "has(hop.route) && hop.route == 'answer' && has(context.channel_node) && context.channel_node == 'chat'",
    "modifier": {"set_hop": {"route": "'in_answer'"}} }
]
```

The down-edge is not decoration: without it every answer of this channel dead-letters
with `no_route`. It ends here, and the app hears the answer elsewhere.

The app's own edge is the third, and it replaces the one that used to reach the firewall:

```json
{ "from": "./apps/chat", "to": "./channels/chat",
  "condition": "has(hop.route) && hop.route == 'typed'",
  "modifier": {"set_hop": {"route": "'in_typed'"}} }
```

## Its own talky

A channel has its own talky (`display-hive.md` § 8.2: one talky per channel, one identity,
one tone). `assistant` ships the second one, `./talky-chat`, and splits `in_turn` on
the `context.channel_node` the up-edge above stamps -- so nothing here names a talky and
nothing here changes when a third channel grows one.

## Driving it

```bash
echo '{"envelope": {"header": {"hop": {"route": "in_typed"}, "context": {}}},
       "body": {"messages": [{"origin": "user", "type": "text", "text": "hello"}]},
       "params": {"user_id": "4711"}}' \
  | python3 templates/chat-channel/channel.py
```

The chat app that fills the input line lives outside this repository, in `voice2vision`.
