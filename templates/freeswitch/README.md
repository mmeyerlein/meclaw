# `freeswitch@2.0.3`

A telephone as one **channel** of a person, in two halves inside one hive.

**It succeeds `phone@1.0.0`, a name that no longer exists.** Same template: `phone` said the medium
where it should have said the machine. What is behind this channel is a
FreeSWITCH, and the day a colony wants a second telephony edge -- a Twilio, an
Asterisk -- the two have to be able to stand beside each other under names that
say which is which. The **node** is the machine (`<member>/channels/freeswitch`,
`context.channel_node = 'freeswitch'`); the **conversation** is still a
telephone (`context.channel = 'phone'`, `hop.platform = 'phone'`), because that
is a kind of room and not a vendor. Migration for a colony that already has the
old node: the migration is § *The migration from `phone@1.0.0`* at the end.

* the **media half** is a `voice` cell: it answers on the mount `phone` of the
  colony's one listener, takes audio in and gives audio back, and puts ordinary
  text turns on the topology.
* the **signalling half** is a small state machine — one `code` cell that offers
  the tools, one that keeps the book, one `web_fetch` cell that talks to the
  switch and one `store` that holds the calls.

They live in one hive because a call is one thing, and because **one id binds
both halves**: FreeSWITCH's channel UUID is the `?session=` of the audio fork,
and therefore the `call_id` an answer is spoken back into. Since 1.1.0 both
halves stamp that id as `hop.call_id` and the answer comes back on
`context.call_id`; `session_id` is what a member's `session-keeper` mints, and
the two are no longer the same word ([#620](https://github.com/mmeyerlein/meclaw/issues/620)). A colony that kept
the two apart would have to invent a second table to say which socket belongs to
which number.

**FreeSWITCH stays the media edge, in gateway mode.** It does the SIP, it owns
the trunk, and `mod_audio_stream` connects to the media half as a WebSocket
*client*. Nothing in this template speaks SIP, and nothing in it holds a trunk.

## The cells

| path | type | from |
|---|---|---|
| the template root itself | `hive`, `ports: []` | the address. The two connect points are the one exception this template pronounces about itself |
| `voice` | `ref` | the `voice` template — the media half, with `audio_out_frame_ms: 20` and `emit_speak_end: true` ordered here |
| `dial` | `code` | the offer: `call`, `hangup` and the three line tools, and the menu entry that makes them callable |
| `signal` | `code` | the book: every lane becomes a store bundle, and the store's answer becomes a turn, a command at the switch, or a tool result |
| `gateway` | `web_fetch` | the one command channel to `mod_xml_rpc` |
| `calls` | `store` | the calls this channel has going |

## What travels

| direction | what travels |
|---|---|
| in, `in_speak` | the finished assistant turn, to be spoken into the call. `context.call_id` picks the connection, exactly as it does for a `voice` channel standing on its own |
| in, `call_incoming` | somebody is ringing this member. `hop` carries `call_uuid`, `number` and — since 2.0.0 — `user_id`, the member the switch put the caller through as. This is THE turn of an inbound call — a call with no `user_id` and no `callers` entry gets no turn and its leg is put down, and since 1.1.0 a call that arrives while the line is busy gets what `params.second_call` says it gets (§ *What a second call gets*) |
| in, `call_ringing` | the switch is ringing a number this channel dialled. It moves the row and raises no turn |
| in, `call_answered` | somebody picked up. `hop` carries `call_uuid`. A turn for a call this channel PLACED; for an inbound call it moves the row and raises no turn, because `call_incoming` already said it |
| in, `call_ended` | the line is down, from either end. `hop` carries `call_uuid` and `cause`. It moves the row and raises no turn |
| in, `tool` / `schemas` | a call to `call`/`hangup`, and the menu tick. Both dock on `<freeswitch>/dial` |
| out, `hop.route == 'turn'` | one thing that was said, or one thing that happened to the line. `hop` carries `session_id` and `call_id`, `turn_id` (`<session_id>#<n>`), `platform` (`phone`), `number`, `user_id`, and `call_state` (`incoming`, `answered`, `busy`, `no_answer`, `failed`) for a turn about the line rather than about words |
| out, `hop.route == 'partial'` | an interim transcript, out of the media half. OFF by default — `emit_partials` on the `voice` cell turns it on |
| out, `hop.route == 'error'` | a caller with no entry in `callers` — whose leg is put down in the same breath — a request this hive cannot read, or the media half's own failure |
| out, `tool_result` / `tool_schemas` | the receipt of a `call`/`hangup`, and the offer itself |
| out, `call_accepted` / `call_queued` / `call_refused` / `call_abandoned` | what a second call got. One receipt per inbound call, empty `messages[]`, `hop` carries `call_id`, `policy`, `capacity` and `cause`. The installing manifest draws the edge that drains them |

**One lane travels only INSIDE this hive**, and it is the reason the media half
carries an `override_params` at all: `speak_end`, from the `voice` cell to the
signalling half, one per sentence the assistant spoke. Nothing outside sees it.
What it buys is two things a telephone needs and a browser does not — a hang-up
that waits for the sentence, and a barge-in that reaches the switch — and both
are written out under *Hanging up* below.

**A fact about the line becomes a TURN when a person could answer it**, and
that is the whole reason this is a channel and not an app. A call that came in,
a call this channel placed and somebody picked up, a call nobody took: they are
things that *happen*, they are not values a function returns, and an assistant
learns about them the way it learns about anything else somebody says to it.
`hop.call_state` says which of them it is, and the ingress edge promotes it into
`context.call_state` — a hop survives one delivery, and a turn of this channel
goes through the member's screen before it reaches an assistant, so the stamp has
to become a context key or it is gone by the time anybody could read it. Spoken
words carry an empty one.

**And a fact nobody can answer is BOOKED, not said** — the one thing 1.0.1
changed (GH #614). *Every* fact used to become a turn, and the first real
inbound call showed what that costs, twice in the same minute:

* the dialplan answers an inbound leg **at once**, so `call_incoming` and
  `call_answered` reached this hive inside the same second and raised two turns
  that said the same thing. The assistant answered both and **the caller heard
  two greetings**. So the announcement of an inbound call is
  `call_incoming` — the moment a person is on the line — and the
  `call_answered` behind it moves the row and says nothing. Since `2.0.2` it
  also **sounds** like that moment: the turn used to read *“Incoming call from
  …”* with `call_state: incoming`, the assistant read it as a notice to the
  owner and asked *“Shall I pick up?”* into a line the caller was already on.
  It now reads *“The caller is on the line. Greet them.”* with
  `call_state: live` — no question, and nobody named in the text: a turn is
  answered, so a number or a member id inside it is read back to the caller.
  Both travel on the hop (`hop.number`, `hop.user_id`), where the member's
  firewall reads them (GH #665);
* `call_ended` raised a third turn, and the sentence that argued for it — *an
  agent that goes on talking into a call that ended is the failure this lane
  exists to prevent* — is exactly the failure it caused. **A generation reads a
  turn by answering it.** The assistant said goodbye into a line that was
  already down (`in_speak` → `no live connection`), called `hangup` on a call
  that was no longer running (*"no call is running"*), and said goodbye again.

The second one is ADR-0025's (`plans/adr/0025-a-receipt-is-feedback-to-a-writer.md`)
reasoning one lane over: *a receipt is feedback to a writer, never a turn of a
person* — because **a model answers everything it is handed**. The end of a call
is not something a person said, and the line an answer would go into is the very
thing that ended. Where it IS written down is the book: `calls` carries the
row's `state` and its `cause`, and that is the record of the line this hive has
always kept. `call_ringing` was the shape all along.

## The five tools

Two are about a **call**, three are about the **line** — which numbers reach
this member at all, and what a caller types before they do.

```
call(number, purpose)   ring somebody up. `number` in international form,
                        `purpose` one short line about why.
hangup()                end the call that is running. If the assistant is in
                        the middle of a sentence, the line stays up until that
                        sentence is finished -- see *Hanging up* below.

add_number(number)      let that number through to this line. It rings nobody.
set_pin(pin)            ask every caller for those 4-8 digits first.
disable_pin()           stop asking.
```

The three line tools write **the switch's own table** and read nothing back
(§ *What leaves for the switch*). Their receipt says which command **left**, the
way a `call` receipt says which number is ringing rather than that somebody
picked up. A switch that refuses the write — no `mod_db`, a wrong realm, an
XML-RPC that answers `401` — comes back as one refusal on this channel naming
the operation (`line_write_failed`), so a row that was not written is not a
silence.

`call` answers **immediately**, with the session the conversation will run
under — not when the telephone is picked up. The outcome arrives as a turn
(`answered`, `busy`, `no_answer`, `failed`), because that is what it is. A tool
that waited for it would hold a round open across a ringing telephone, and the
fan-in's idle window is not a ring timer.

There is **no clock in this template**. The ring timeout travels to the switch as
FreeSWITCH's own `originate_timeout` (`ring_timeout_ms`, default 45 s), and the
`originate` answer comes back over the same connection. Nothing polls anything.
The one place a clock would be defensible is the hang-up that waits for a
sentence, and it is deliberately absent there too — *What is not here* says why.

## Who is on the line

**The switch says so, and since 2.0.0 that is the first answer.** A
`call_incoming` carrying `hop.user_id` is trusted: the dialplan asked for the
PIN, looked the number up in the switch's own table and stamped the member it
found there. The switch is the proxy, and an identity a proxy verified is the
identity of that call — the number is never read as one while a stamp is there.

`params.callers` on `./signal` is the **fallback**, for a dialplan that stamps
nothing. It maps **number → sender id**:

```json
{"callers": {"+493012345678": "alex", "+491701234567": "robin"}}
```

Either way the id becomes `hop.user_id` on every turn the signalling half
raises, and the member's firewall allowlists senders by exactly that key. **A
call with neither** — no stamp and no entry — is refused with `unknown_caller`:
one `error` out of the rim, and no turn. That is deliberate: a turn without a
sender is a turn a member that allowlists one person refuses anyway, and a
refusal that says *which* number rang is worth more than a rejected turn.

**And the refused leg is put down** (GH #614): one `uuid_kill` on the UUID the
dialplan named, in the same breath as the refusal. Refusing and doing nothing
else left the caller listening to an **answered** call that nobody would ever
speak into — the dialplan had already answered the leg in order to `curl` this
hive, and its own fallback fires on a `curl` that *fails*, not on one that comes
back with a refusal inside it. This hive owns nothing about what happens next: it
ends the leg it is refusing and hands the extension back, and whether the caller
then hears a mailbox, a tone or a busy signal is the dialplan's to write.

The turns of the **media half** — the words actually spoken during a call —
carry no sender of their own, because the `voice` cell has no idea who is on the
line. For a personal agent that speaks with one person, the ingress edge promotes
that person as a literal, exactly as `templates/voice/README.md` writes it. A
channel serving several callers wants the media half's turns to carry the sender
of their session; that is a lane through the signalling half and it is not built
(see *What is not here* below).

## Wiring it into a member

**A phone channel costs one node and two edges**, the same shape a `voice`
channel costs — plus, if the assistant should be able to *place* calls, the two
tool v-lanes and their way back.

```json
{"scope": "<member>", "diff": {
  "add_nodes": [{"name": "channels/freeswitch", "template": "freeswitch@2.0.3",
                 "override_params": {
                   "signal": {"dial_prefix": "sofia/gateway/fs02/",
                              "voice_ws_url": "ws://<colony-host>:<listener-port>/phone/ws",
                              "caller_id_number": "<the number this member calls from>",
                              "line_user_id": "<the person's sender id>",
                              "callers": {"<a number>": "<the person's sender id>"}}}}],
  "add_edges": [
    {"from": "./channels/freeswitch", "to": "./channels",
     "condition": "has(hop.route) && (hop.route == 'turn' || hop.route == 'partial' || hop.route == 'error')",
     "modifier": {"set_context": {"channel_node": "'freeswitch'",
                                  "channel": "'phone'",
                                  "assistant": "'<assistant>'",
                                  "audience_set": "'[\"agent:<assistant>\",\"member:<member>\"]'",
                                  "user_id": "has(hop.user_id) && hop.user_id != '' ? hop.user_id : '<the person's sender id>'",
                                  "call_state": "has(hop.call_state) ? hop.call_state : ''",
                                  "call_id": "has(hop.call_id) ? hop.call_id : ''",
                                  "session_id": "has(hop.session_id) ? hop.session_id : ''"}}},
    {"from": "./channels", "to": "./channels/freeswitch",
     "condition": "has(hop.route) && hop.route == 'answer' && has(context.channel_node) && context.channel_node == 'freeswitch'",
     "modifier": {"set_hop": {"route": "'in_speak'"}}},

    {"from": "./channels/freeswitch", "to": "./channels",
     "condition": "has(hop.route) && (hop.route == 'call_accepted' || hop.route == 'call_queued' || hop.route == 'call_refused' || hop.route == 'call_abandoned')",
     "modifier": {"set_context": {"channel_node": "'freeswitch'", "channel": "'phone'"}}},
    {"from": "./channels", "to": ".",
     "condition": "has(hop.route) && (hop.route == 'call_accepted' || hop.route == 'call_queued' || hop.route == 'call_refused' || hop.route == 'call_abandoned')",
     "modifier": {"set_hop": {"route": "'error'", "kind": "hop.route"}}},

    {"from": "./channels/freeswitch", "to": "./channels",
     "condition": "has(hop.route) && (hop.route == 'tool_result' || hop.route == 'tool_schemas')",
     "modifier": {"set_context": {"tool_answerer": "'freeswitch'",
                                  "call_id": "has(hop.call_id) ? hop.call_id : (has(context.call_id) ? context.call_id : '')",
                                  "session_id": "has(hop.session_id) ? hop.session_id : (has(context.session_id) ? context.session_id : '')"}}},
    {"from": "./assistants/<gen>/talky", "to": "./channels/freeswitch/dial", "lane": "tool",
     "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && (hop.tool_name == 'call' || hop.tool_name == 'hangup' || hop.tool_name == 'add_number' || hop.tool_name == 'set_pin' || hop.tool_name == 'disable_pin')",
     "modifier": {"set_context": {"tool_caller": "'talky'", "assistant": "'<gen>'"},
                  "delete_context": ["col_phase", "consult_class", "consult_id", "tool_answerer"]}},
    {"from": "./assistants/<gen>/talky", "to": "./channels/freeswitch/dial", "lane": "schemas",
     "condition": "has(hop.route) && hop.route == 'schemas'",
     "modifier": {"set_context": {"tool_caller": "'talky'", "assistant": "'<gen>'"},
                  "delete_context": ["col_phase", "consult_class", "consult_id", "tool_answerer"]}}
  ]}}
```

**Every promotion in that manifest is `has(...)`-guarded, and one of them was
not** (GH #603, defect 1): the third edge wrote
`has(hop.session_id) ? hop.session_id : context.session_id`, and on the very
first tool call — the menu tick, which carries no session at all — the *else*
branch read a key that was not there. A CEL modifier that fails to evaluate
**skips the whole edge**, so the offer never reached the assistant's menu and
nothing said why. The rule has no exception: **every** key read off `hop` or
`context` inside a modifier is guarded, even the one that "obviously" exists.

Read it in four groups.

**The channel half** is the first two edges, and they are the `voice` channel's
own two, word for word (`templates/voice/README.md` § *Wiring it into a member*):
one promotion up, one restamp down. `call_id` is promoted on **all three**
outbound lanes and not only on `turn`, because a failure that happened inside a
call carries that call, and an edge that promoted the call only on the happy
path would make every failure look like it came from nowhere. `user_id` is the
one line the `voice` channel writes as a bare literal and this one writes as a
fallback: the signalling half knows the caller, the media half does not.

**`voice_session` is gone, and the call has a key of its own** (retracted in
1.1.0, [#620](https://github.com/mmeyerlein/meclaw/issues/620)). Until 1.0.1 the
manifest carried two extra lines, and this section described them like this:

> The media half selects the connection it speaks into by `context.session_id`.
> Between the channel and the assistant stands the member's `session-keeper`,
> whose whole job is to mint and stamp a session of its OWN on that same key —
> so the answer came back carrying the keeper's generation id, no connection
> held it, and the cell answered `unknown_session` while the caller listened to
> silence.

The diagnosis was right (GH #603 § 3) and the cure was a workaround: a second
context key, `voice_session`, written on the way up and put back on `session_id`
on the way down. It was written down as a workaround because *which of the two
owns `context.session_id` is a ruling nobody has made*.

The ruling has been made: **the call key belongs to the channel.** Both halves
stamp `hop.call_id`, the media half selects a connection by `context.call_id`,
and `context.session_id` stays the keeper's. So the manifest promotes `call_id`
on the way up and restamps **nothing** on the way down — there is no longer
anybody to take the key away. A colony that is still on the old pair does not
have to move on the same day, since the `voice` cell reads `context.session_id`
wherever `call_id` is absent and prefers `call_id` where both are there.

**The receipt half** is the two edges after that, and they exist because a
channel now says what a second call GOT (§ *What a second call gets*). The four
lanes leave the hive under their own names; the member has one lane for
something that happened and nobody inside consumes, and that lane is `error` —
so the second edge restamps them onto it with the original lane on `hop.kind`.
That is not an invention: it is what this level already does with a `receipt` no
app of the member owns (ADR-0025), spelled out in the manifest instead of shipped
in the template. **Say the cost out loud: every accepted call leaves an `error`
line at the member's rim**, not only a refused one — the lane is the member's one
exit for something that happened and nobody inside consumes, so an operator
reading that lane sees one entry per inbound call with `hop.kind` naming which
kind it was. Whoever wants them apart from real failures draws the first edge to
a sink of their own instead of to `.`. **A manifest that draws neither edge gets `no_route`, once per
inbound call** — the rule the `partial` lane already follows. An operator who
wants the receipts somewhere else (an app, a log sink) draws that edge instead.

**The offer half** is the last three, and it is the app rim's mechanism at a
second rim (ADR-0024, ADR-0020). Since 1.6.3 the `member` level declares `tool`, `schemas`,
`tool_result` and `tool_schemas` at `./channels` as well as at `./apps`, and
ships the two restamp edges `./channels -> ./assistants` that turn `tool_result`
into `in_tool` and `tool_schemas` into `in_menu` — the same two the apps
container already had. So a channel's tool reaches the brain's menu without the
surface, its collector or the grow recipe being touched at all.

**`emit_partials`** is off in the media half. A screen or an app that wants the
draft of a sentence orders it (`override_params: {"voice": {"emit_partials":
true}}`), the rule a `voice` channel already follows. **`emit_speak_end` is ON**,
and it is ordered by the template rather than by the manifest, because the edge
that drains it is one of this hive's own — see *Hanging up* below.

## What a second call gets

Until 1.0.1 the answer was *nothing decides*: a second `call_incoming` from an
allowlisted number became a second row and a second turn, both calls fed one
conversation, and `hangup` ended whichever was newest. `params.second_call` is
the decision, and `params.capacity` is how many calls it allows at once.

| `second_call` | what an inbound call gets while the line is busy |
|---|---|
| `busy` (default) | refused. The leg is ended at the switch with `uuid_kill <uuid> USER_BUSY`, so the caller hears a busy signal rather than a line that died. No turn |
| `queue` | taken and held. Its audio fork is stopped, `params.queue_hold_media` plays into the leg, and it runs when the call in front of it ends. No turn until then |
| `parallel` | a session of its own, up to `capacity`. Over that it is refused exactly as `busy` refuses |

`capacity` is read for **every** policy, so `busy` is `parallel` with
`capacity: 1` and the decision is one comparison rather than three branches. It
counts the live states (`dialing`, `ringing`, `answered`); a caller waiting in
the queue occupies nothing.

**Every outcome leaves a receipt**, in the book and on a lane:

| lane | when | `hop.cause` |
|---|---|---|
| `call_accepted` | the call is on the line | empty, or `dequeued` when it waited first |
| `call_queued` | it is waiting and hearing hold media | empty |
| `call_refused` | capacity was full under a policy that refuses | `busy` |
| `call_abandoned` | it hung up while waiting | the switch's own hangup cause |

Each carries `hop.call_id`, `hop.policy` and `hop.capacity`, and each leaves the
hive. The installing manifest draws the edge that drains them (§ *Wiring it into
a member*); without it they dead-letter as `no_route`, once per inbound call.

**A waiting caller is taken off the recogniser, and that is the point.** The
dialplan answers an inbound leg before it `curl`s this hive, and it starts the
audio fork there — so a caller parked in a queue would be talking into a
recogniser, and every sentence would become a turn of a conversation that has
not started. Queueing therefore sends `uuid_audio_stream <uuid> pause` and
`uuid_broadcast <uuid> <queue_hold_media> aleg`; promotion sends
`uuid_break <uuid> all` and `uuid_audio_stream <uuid> resume`. Each pair leaves
in ONE bundle and the two commands of a pair reach the switch in **either**
order — see *What is not here*. No synthesis
provider is involved, so a queue costs nothing at a vendor. `queue_hold_media`
is `local_stream://moh` by default and takes any `uuid_broadcast` source, so a
recorded announcement is a `file_string://…`.

**`pause`/`resume` and not `stop`/`start`, and the reason is the socket.** Both
pairs are dispatched by the module — `mod_audio_stream.c` v1.0.3 lines 148-186
list `start`, `stop`, `pause`, `resume`, `send_text` — but `stop` closes the
WebSocket, which ends the media half's session. The promotion would then have to
compose a fresh `start` out of THIS hive's knobs (`voice_ws_url`,
`fork_sample_rate`) and hope they still match what the dialplan started the fork
with; an inbound fork is the dialplan's, not this channel's. A pause leaves the
socket, the session and the call's id exactly where they are, so a promotion is
one command and reconstructs nothing.

**The queue is emptied by the end of a call and by nothing else.** Every
transition of a row to `ended` — the far end's `call_ended`, and the `uuid_kill`
this channel fires itself — asks the table for the oldest `queued` row and
promotes it. There is no timer and nothing polls. Two ends arriving at once cost
one harmless duplicate: the promoting update names the state as well as the
uuid, so the second one matches nothing, and the second `resume` reaches a
stream that is already running.

> **To be verified at the switch (fs02).** What a `resume` on a stream that was
> never paused answers is not established here: the module's dispatch table is
> readable, its per-command behaviour is not, and no recording of that case
> exists. The claim this section rests on is only that the second `resume` is
> harmless, because the first one already did the work — and the switch pass
> discards its answer either way, since it is not an `originate`.

**Two conversations, or one?** `context.channel` is what the holders count by —
`session-keeper` opens one generation per value, `firewall` rate-limits one
bucket per value, `memory-hive` writes it down as the room a thing was said in
(`templates/member/README.md` § *The two channel keys*). The manifest above
ships `'phone'`: one line, one room, one rate bucket, and a redial is the same
conversation, which is what a personal agent wants. Under `parallel` that is
wrong — two callers would share one generation and the assistant would read one
interleaved history — so a manifest that sets `parallel` with a capacity above
one changes that one line:

```json
"channel": "has(hop.call_id) && hop.call_id != '' ? 'phone:' + hop.call_id : 'phone'"
```

**Both halves or neither.** What it buys: two calls are two generations, so
neither caller reads the other's history. What it costs, said out loud: the
memory writes the room down per call rather than per line, and the rate limit
becomes per call — a redial is a fresh bucket. That is why it is not the
default, and why it is one line rather than a param.

**What is NOT arbitrated** is an outbound `call`: the model asks for a line and
gets one, whatever else is running. `second_call` is about who is allowed to
ring THIS member, and a colony that wants to stop its own agent dialling twice
has a tool schema to say so in, not a policy.

## What the dialplan owes

FreeSWITCH tells the channel what happened over the **front door** — one `curl`
per event, addressed at the hive itself. No member edge is involved: the hive
path *is* the address, and its own rim routes the event onwards.

```
POST /messages
{"target": "<member>/channels/freeswitch",
 "hop": {"route": "call_answered",
         "call_uuid": "<the FreeSWITCH channel uuid>",
         "number": "<the other end, international form>",
         "cause": ""},
 "body": {"messages": []}}
```

| `hop.route` | when the dialplan sends it | what the channel does |
|---|---|---|
| `call_incoming` | an inbound leg arrives, and the dialplan has already answered it | one turn *“The caller is on the line. Greet them.”* — the caller and the member ride on `hop.number`/`hop.user_id` and are named nowhere in the text — with `call_state: live` — **if the line is free, or the policy takes it** (§ *What a second call gets*); a queued or refused call raises no turn and leaves a receipt instead. `hop.user_id` is the member the switch put the caller through as and is trusted where it is there. For a call with neither that nor a `callers` entry: one `unknown_caller` error **and** a `uuid_kill` on that leg, before the policy is ever asked |
| `call_ringing` | an outbound leg starts ringing | moves the row, raises nothing |
| `call_answered` | either leg is answered — the same place the audio stream is started | for a call this channel PLACED: one turn *“… answered. Purpose of this call: …”*. For an INBOUND one: moves the row, raises nothing — `call_incoming` was the turn |
| `call_ended` | the leg hangs up | moves the row, `state` and `cause`, and raises nothing |

`call_uuid` is the **same** UUID that hangs on the stream URL as `?session=`. For an
outbound call the channel mints it and hands it to the switch as
`origination_uuid`, so both ends of the wire already agree before the telephone
rings.

**Four extensions serve every line of every colony, because the table is the
register.** The switch is the proxy: it holds the numbers, it asks for the PIN, it
decides which colony and which door a caller reaches, and it stamps the member it
found. This colony holds none of that. The rows are written by the three line
tools (§ *What leaves for the switch*) and read here, once per call — a number row
carries `<user_id>|<stream URL>`, so **adding a colony is adding a row**:

```xml
<!-- dialplan/public/00_meclaw.xml -->
<extension name="meclaw_line">
  <!-- FIRST the dialled number, so the re-hunts below do not land here. -->
  <condition field="destination_number" expression="^\+?\d+$"/>
  <!-- Then the row, by the PAIR: the realm is the number that was dialled, the
       key is the number that is calling, the value <user_id>|<stream URL>.
       Actions AND anti-actions are children of this condition, the LAST one:
       FreeSWITCH collects both as children of a condition and silently ignores
       anything that sits at extension level. -->
  <condition field="${db(select/meclaw_lines_${destination_number}/${caller_id_number})}"
             expression="^([^|]+)\|(.+)$">
    <action application="set" data="meclaw_user=$1" inline="true"/>
    <action application="set" data="meclaw_ws_url=$2" inline="true"/>
    <action application="set" inline="true"
            data="meclaw_wants=${db(select/meclaw_lines_${destination_number}/pin.${meclaw_user})}"/>
    <action application="answer"/>
    <action application="sleep" data="300"/>
    <action application="execute_extension" data="meclaw_ask XML ${context}"/>
    <anti-action application="answer"/>
    <anti-action application="playback" data="ivr/ivr-call_cannot_be_completed_as_dialed.wav"/>
    <anti-action application="hangup" data="CALL_REJECTED"/>
  </condition>
</extension>

<extension name="meclaw_ask">
  <condition field="destination_number" expression="^meclaw_ask$"/>
  <!-- A line with a PIN: ask for it (three tries, eight seconds each), then
       hunt again with the digits in hand. A line without one goes straight on,
       and that hand-over is this condition's anti-action. -->
  <condition field="${meclaw_wants}" expression="^.+$">
    <action application="play_and_get_digits"
            data="4 8 3 8000 # ivr/ivr-please_enter_pin_followed_by_pound.wav ivr/ivr-that_was_an_invalid_entry.wav meclaw_pin ^\d{4,8}$"/>
    <action application="execute_extension" data="meclaw_check XML ${context}"/>
    <anti-action application="execute_extension" data="meclaw_connect XML ${context}"/>
  </condition>
</extension>

<extension name="meclaw_check">
  <condition field="destination_number" expression="^meclaw_check$"/>
  <!-- Compared as TEXT. `${cond(a == b ? …)}` compares numerically where both
       sides look numeric, so a stored `0123` would be satisfied by `00123`.
       Both variables are cleared on BOTH paths: a channel variable is in the
       XML CDR of this call. -->
  <condition field="${meclaw_pin}" expression="^${meclaw_wants}$">
    <action application="unset" data="meclaw_wants"/>
    <action application="unset" data="meclaw_pin"/>
    <action application="execute_extension" data="meclaw_connect XML ${context}"/>
    <anti-action application="unset" data="meclaw_wants"/>
    <anti-action application="unset" data="meclaw_pin"/>
    <!-- A file of its own, not the one `play_and_get_digits` plays: that one
         says "those digits are not a PIN-shaped number", this one says "that
         was not the PIN". A caller who hears the same sentence for both learns
         nothing from either. -->
    <anti-action application="playback" data="ivr/ivr-access_denied.wav"/>
    <anti-action application="hangup" data="CALL_REJECTED"/>
  </condition>
</extension>

<extension name="meclaw_connect">
  <condition field="destination_number" expression="^meclaw_connect$"/>
  <!-- The API answers on the same listener as the stream, so its address comes
       out of the same row: the authority is the capture, and the scheme is the
       PLAINTEXT one the param default spells (`ws://host:port/mount/ws`). A
       `wss://` listener wants `https` here and a second extension for it. A URL
       with no path does not match at all, and then the call is refused rather
       than posted to a nonsense address. -->
  <condition field="${meclaw_ws_url}" expression="^ws://([^/]+)/.+$">
    <action application="set" data="meclaw_api=http://$1/messages"/>
    <action application="set"
            data="api_result=${uuid_audio_stream(${uuid} start ${meclaw_ws_url}?session=${uuid}&amp;sample_rate=8000 mono 8000)}"/>
    <!-- `content-type application/json` is not optional: without it mod_curl
         posts form-encoded and `POST /messages` answers 415. -->
    <action application="curl"
            data="${meclaw_api} content-type application/json post {&quot;target&quot;:&quot;<member>/channels/freeswitch&quot;,&quot;hop&quot;:{&quot;route&quot;:&quot;call_incoming&quot;,&quot;call_uuid&quot;:&quot;${uuid}&quot;,&quot;number&quot;:&quot;${caller_id_number}&quot;,&quot;user_id&quot;:&quot;${meclaw_user}&quot;},&quot;body&quot;:{&quot;messages&quot;:[]}}"/>
    <action application="park"/>
    <anti-action application="playback" data="ivr/ivr-call_cannot_be_completed_as_dialed.wav"/>
    <anti-action application="hangup" data="CALL_REJECTED"/>
  </condition>
</extension>
```

**The walk, once, because the order is the whole trick.** A hunt evaluates
**every** condition of an extension before it runs a single action, and it runs
the collected actions afterwards — so a `set` that a later condition has to read
carries `inline="true"` and runs during the hunt. Actions and anti-actions are
collected as children of a **condition**; at extension level they are dropped
without a word, which is why every block above sits inside its extension's last
condition. `execute_extension` starts a **fresh hunt** with `destination_number`
set to the extension it names, which is why each one tests that name first:
without it the re-hunt would match `meclaw_line` again — the row is still there
and `caller_id_number` has not changed — and the call would loop through
`answer`/`sleep` for ever. The compare cannot be part of the asking hunt, because
nobody has typed anything when its conditions are evaluated. The four paths:

* **No row.** `meclaw_line`'s second condition fails: answer, announcement,
  `CALL_REJECTED`. Nothing else is reached.
* **A row without a PIN.** `meclaw_line` sets the three variables, answers and
  hands over; `meclaw_ask`'s second condition finds `meclaw_wants` empty and its
  anti-action hands over to `meclaw_connect`, which starts the stream and posts
  the `call_incoming`.
* **A row with a PIN, entered right.** `meclaw_ask` asks and hands over;
  `meclaw_check` matches, clears both PIN variables and hands over to
  `meclaw_connect`.
* **A row with a PIN, entered wrong.** `meclaw_check`'s condition fails: its
  anti-actions clear both variables, play `ivr/ivr-access_denied.wav` and hang
  up with `CALL_REJECTED`. That is a different file from the one
  `play_and_get_digits` plays on a malformed entry
  (`ivr/ivr-that_was_an_invalid_entry.wav`), because the two refusals are two
  different things: the digits were not PIN-shaped, or they were not the PIN.

**The row is addressed by the pair (dialled number, caller number), and the
realm carries the first half** (GH #667). `mod_db` addresses a row by realm and
key, so the block above reads
`db(select/meclaw_lines_${destination_number}/${caller_id_number})`: one realm
per number the switch answers on, named `meclaw_lines_<dialled number>` — the
number as it stands in `destination_number` on that switch, `+` included if the
trunk delivers one, since the first condition above admits both and the realm
has to match it character for character — and inside it the caller's number as
the key. Both directions fall out of that without a change to any cell: one
caller reaches different colonies on different numbers, and one number serves
many callers on many colonies. A colony behind such a switch names the realm of
the line it is reached on in `params.db_realm`, so that `add_number`, `set_pin`
and `disable_pin` write into the realm the dialplan reads for that number
(§ *What leaves for the switch*); the PIN row sits in the same realm, which is
why the second `db(select/…)` above names it too. One hive names one realm, so
one hive is one line: a colony reached on two numbers behind such a switch
carries two `freeswitch` nodes, one per number.
The shipped default, `meclaw_lines`, is the one-number case — one switch,
one number, and a block that reads `db(select/meclaw_lines/${caller_id_number})`
instead, keyed by the caller alone, because with one number there is nothing
else to tell apart. With the default realm, replace
`meclaw_lines_${destination_number}` by `meclaw_lines` in both lookups of the
block above — or set `db_realm` to `meclaw_lines_<dialled number>` and keep the
block as it stands. Copied as it stands against the default realm, the block
selects an empty row on every call and hangs up with `CALL_REJECTED`, and
nothing in the log says why. `freeswitch@2.0.2` shipped the caller-only form as
the block itself, so one caller number reached one colony whichever number they
had dialled.

**What is measured and what is not.** The rows, the three tools that write them
and the `user_id` this hive trusts are measured
(`freeswitch_channel_places_a_call_and_hears_the_line.rs`). The XML above is read
against FreeSWITCH's own parsing rules and **not** against a running switch: it
is the shape to start from, not a tested configuration. Five of those rules are
what it is built on — every condition of a hunt is evaluated before any action
runs unless the action carries `inline="true"`; actions and anti-actions are
collected as children of a condition and ignored anywhere else; `execute_extension`
starts a fresh hunt with `destination_number` set to the extension it names, which
is why each one guards on that name first; `$1`…`$9` are the capture references and
`${1}` is a channel variable; and `mod_curl`'s form is
`curl <url> content-type <mime> post <data>`, without which the post is
form-encoded and `POST /messages` answers `415`. A switch serving two colonies
needs no second variable anywhere: both colonies' rows sit in the same table —
one realm when they share a number, one realm per number when they do not — and
each names its own listener.

**If meclaw does not answer, the dialplan falls back to the mailbox.** That
belongs in the dialplan and not here — a `curl --max-time` and a condition on its
exit code; a person who rings a colony that is down should hear a mailbox, not
silence. The **refusal** of a known-good colony is the case that exit code does
not cover — the `curl` succeeds and carries a refusal — which is why this hive
kills that leg itself rather than leaving it parked (see *Who is on the line*).

## What leaves for the switch

Six commands, all as a GET at `mod_xml_rpc`'s `/webapi` endpoint — `web_fetch`
implements GET and nothing else (`docs/cell-types.md` § `web_fetch`), and
`/webapi/<command>?<args>` is exactly a GET. The endpoint, its credentials and
its host live in **one** provider lane, `${FREESWITCH_XMLRPC_BASE_URL}`, which is
the only `${…}` token in this template. The count is worth reading before you
write an ACL in front of that endpoint:

```
GET <base>/webapi/originate?{origination_uuid=<uuid>,originate_timeout=45,
      ignore_early_media=true,
      api_on_answer='uuid_audio_stream <uuid> start <voice_ws_url>?session=<uuid>&sample_rate=8000 mono 8000'}
      <dial_prefix><number> &park()
GET <base>/webapi/uuid_kill?<uuid>
GET <base>/webapi/uuid_kill?<uuid>%20USER_BUSY
GET <base>/webapi/uuid_break?<uuid>%20all
GET <base>/webapi/uuid_audio_stream?<uuid>%20pause
GET <base>/webapi/uuid_audio_stream?<uuid>%20resume
GET <base>/webapi/uuid_broadcast?<uuid>%20<queue_hold_media>%20aleg
GET <base>/webapi/db?insert/<db_realm>/<number>/<line_user_id>|<voice_ws_url>
GET <base>/webapi/db?insert/<db_realm>/pin.<line_user_id>/<pin>
GET <base>/webapi/db?delete/<db_realm>/pin.<line_user_id>
```

Ten lines, six commands. `uuid_kill` appears twice because a refused call is
ended with a CAUSE — `USER_BUSY` is what the caller hears as a busy signal, and
a bare kill is a line that died for no reason they can name — `uuid_audio_stream`
twice for the pause and the resume of a waiting caller, and `db` three times
because a line has three things somebody can say about it.

**The last three are the line tools, and they write the SWITCH's table.**
`mod_db` takes `db insert/<realm>/<key>/<value>` and `db delete/<realm>/<key>`,
so `/webapi/db?<args>` is that command as a GET. Two shapes of row live in the
realm `params.db_realm` (`meclaw_lines`, or `meclaw_lines_<dialled number>`
behind a switch that answers on more than one number):

* one keyed by a **number**, carrying two fields —
  `<line_user_id>|<voice_ws_url>`: who the caller is put through as, and the
  whole stream URL the switch is to connect to. Host and mount are inside it, so
  **one row is everything the dialplan needs about a line** and adding a colony
  is adding a row rather than a variable in the dialplan.
* one keyed by the **member** under a `pin.` prefix, carrying the digits
  `set_pin` wrote, which `disable_pin` deletes. That is the only place a PIN
  lives: `set_pin(pin)` is told no number, so the key it can address without
  being told one is the member it already knows.

`add_number` needs a number and writes the first; `set_pin` and `disable_pin`
address the second. The prefix is a **dot** and not a slash for the same reason
the URL in a value is safe: `mod_db` splits its command into **four** tokens on
`/` and the value is the last of them
(`switch_separate_string(mydata, '/', argv, 4)` in `mod_db.c`, and
`separate_string_char_delim` stops splitting once the array is full), so slashes
inside a value stay where they are while a slash in a **key** would shift the
value one field along. It is also what keeps the two key spaces apart in one
realm, so a `line_user_id` that reads like a caller number cannot land on a
line.

**The PIN row's value is compared as an anchored regex** —
`expression="^${meclaw_wants}$"` in the dialplan — so it has to be digits and
nothing else. `set_pin` enforces `^\d{4,8}$` for exactly that reason: anchoring
is not escaping, and a `.*` written into the row by hand would satisfy every
entry.

**The PIN is stored as the switch stores it — plain, in its own database, on the
switch host.** That is the proxy's store, the way an `htpasswd` file is nginx's,
and the guard around it is that host's own access control. Hashing it would need
a script module in the dialplan and is not in this version.

**Where else it rests, said out loud.** Three places beside that row, and an
operator should know all three before pointing a switch at this:

* **The channel variables of the call.** The dialplan above puts the expected PIN
  in `meclaw_wants` and the entered one in `meclaw_pin`, and a channel variable
  ends up in the XML CDR of that call. That is why `meclaw_check` clears both on
  **both** of its paths — the compare is over either way, and a CDR file is a
  different exposure from a database row. One path no action reaches: a caller who
  hangs up while the prompt is playing leaves the expected PIN in that call's CDR,
  because the channel is gone before anything can unset it.
* **The colony's message log.** `set_pin` reaches the switch as an ordinary
  `switch` command, so the `db insert` URL — PIN included — is a message like any
  other, and the central message log in `colony.db` keeps messages. The tool call
  the model made carries it too.
* **A realm is persistent: the row is in the switch's database, not in its
  memory.** `mod_db` keeps its rows in the `db_data` table of its own database —
  `call_limit.db` under the switch's `db/` directory by default, or the ODBC DSN
  `db.conf` names — and on load (`do_config` in `mod_db.c`) it creates that table
  only where it is missing and purges `limit_data` alone; nothing touches
  `db_data`. So a line row and a PIN row outlive a restart of the switch and a
  reload of the module, and through `mod_db` a row ends in exactly two ways: a
  `db delete` on its key, or a `db insert` on the same key, which overwrites it
  (the insert is a delete of that key followed by the insert, under a unique
  index on `(data_key, realm)`). What reaches the row past the module — the
  file removed, the DSN pointed elsewhere, a statement run against the backend
  — is the host's business: which backing holds the rows is configured there,
  so check it before treating the table as a register nobody ever has to write
  again.

What this colony does **not** keep is a register: nothing here reads a row back,
and there is no tool that lists one. What it does keep is the call table, which
is a log of calls that happened.

**There is no third field.** A number row is two fields and the PIN lives in the
`pin.` row alone, so there is one place to look and one place to change.

**`api_on_answer`, not `execute_on_answer`, and `uuid_audio_stream`, not
`uuid_audio_fork`** (GH #603, defect 2). Starting the stream is an **API
command**, and `execute_on_answer` runs an **application**: FreeSWITCH answered
`Invalid Application` and hung the freshly answered call up, on the first real
call this template ever made. Two more things about that one line:

- **the rate is written as `8000`, twice, out of one param** (GH #619). A
  telephone call IS 8 kHz. Asking `mod_audio_stream` for `16k` made it hand the
  recogniser interpolated samples that never carried more than 4 kHz of
  bandwidth, and paid twice the bytes for them; Deepgram Flux takes 8000
  natively, so nothing is gained by the detour. `fork_sample_rate` now defaults
  to `8000` and is written into **both** halves of that one line: as the
  module's sampling rate, and as `?sample_rate=` in the fork URL, which is what
  the media half runs its recognition session at. The two places that have to
  agree are therefore one number, and a rate the recogniser does not serve is a
  `400` on the WebSocket naming the ones it does — instead of a pitch nobody
  notices until the transcripts read wrong. Both spellings reach the module —
  `mod_audio_stream.c` v1.0.3 lines 170-177 read `16k`/`8k` by `strcmp` and
  everything else through `atoi` — and the number is what this template writes,
  because the whole `api_on_answer` value is one line at the switch and a digit
  string is the form that cannot be mistaken for a unit; an instance carrying
  `"8k"` is read rather than refused. The module streams L16 and only L16, so
  there is no encoding to choose: `mod_audio_stream` "attaches a media bug and
  starts streaming audio (in L16 format) to the websocket server", and `8k` and
  `16k` are the only two rates it takes.

  **Not yet verified at a switch:** the `&` that now separates `?session=` from
  `&sample_rate=` sits inside the single-quoted `api_on_answer` value. It
  travels percent-encoded through the XML-RPC GET and should reach the switch as
  a literal `&` inside the quotes, but this template's own history says that
  originate strings are read by more parsers than one would like (GH #603), and
  no call was placed for this change — it is marked for verification at the
  switch next to the `uuid_break` note below.
- **the whole variable is left out when `answer_app` starts `&transfer(`.** A
  leg handed to a dialplan extension is that extension's leg, and the extension
  starts its own stream; a second start on the same channel is a second socket
  nobody reads.

`+OK` is **not** the answer to anything: the answer of a call is the dialplan's
own `call_answered`, and one outcome with two producers would arrive twice. A
`-ERR` is read for its reason and becomes `no_answer`, `busy` or `failed` —
**but only for an `originate`**. The other two commands are about a line that
already exists, so their answers are read and dropped; the edge into the gateway
carries `context.phone_op` for exactly that, and without it a `-ERR` from
`uuid_kill` would raise a turn saying a long-answered call had never connected.

## Hanging up, and the sentence that was still running

**Two things in this template wait for the media half to finish speaking**, and
both are the `voice` cell's `speak_end` lane arriving at `./signal` over an edge
inside this hive. Neither exists for a browser, and both exist for a telephone.

**A `hangup` mid-sentence waits.** The book carries a `speaking` **count** per
call — a count and not a flag, because the media half queues what it is given
and two answers in a row are two sentences: it goes up when an `in_speak`
reaches this hive (the same message the media half gets, on a second inner
edge) and down on that call's `speak_end`. A `hangup` that finds it above zero
does **not** call `uuid_kill` — it writes
`hangup_pending` on the row, answers the model *"the line will be cut as soon as
the sentence you are speaking is finished"*, and the `speak_end` that takes the
count to zero is what fires the kill. A `hangup` with nothing being spoken kills
at once, as it always did. Cutting mid-sentence is what the caller hears as the
last word turning into a dial tone.

**Both sides read the row back after they write it, and the symmetry is the
whole point.** A hang-up and the `speak_end` it is waiting for reach the book as
two select-then-write pairs, and they can interleave either way round:

* the hang-up writes its flag **after** the `speak_end` read the row — the count
  is already zero, and only the hang-up's own read-back can see that;
* the `speak_end` writes the count **after** the hang-up read it — the flag is
  already set, and only the `speak_end`'s own read-back can see that.

One read-back closes one of those orders and leaves the other waiting for ever,
which is why there are two. Firing twice is not a risk worth a lock: a second
`uuid_kill` names a call the switch has already dropped, comes back `-ERR`, and
that answer is read and discarded because it is not an `originate`.

**A sentence that was cut short is stopped at the switch.** When the caller
talks over the assistant, the `voice` cell cancels the synthesis and emits
`speak_end` with `reason: cancelled`; when the provider dies mid-sentence it
emits `failed`. Either way the audio the cell had already handed over is at the
far end, and the caller goes on hearing a sentence the assistant has abandoned.
So both reasons fire one command at the switch: **`uuid_break <uuid> all`**.

**Why that command and not one of the module's own.** `mod_audio_stream`
dispatches exactly five subcommands — `start`, `stop`, `pause`, `resume`,
`send_text` (`mod_audio_stream.c` v1.0.3, lines 148-186; anything else answers
`-ERR unsupported mod_audio_stream cmd`) — and its README documents no clear,
flush or interrupt. `uuid_break` is FreeSWITCH's own, out of `mod_commands`:
*"Break out of media sent to channel"*, and with `all` the queue behind the
current item goes too. It says nothing about the stream, so the call keeps
listening while it stops talking.

> **To be verified at the switch (fs02).** This one is reasoned, not read: the
> module's playback half is **closed source** (v1.0.3 is a binary, free up to
> ten concurrent channels), so whether its playback sits in the media path
> `uuid_break` reaches cannot be established from the code. `uuid_break … all`
> is the best-documented command for the job and it is harmless if it misses.
> If it turns out to miss, the fallback with a source behind it is
> `uuid_audio_stream <uuid> stop` plus a restart — which does cut the stream,
> and is why it is not the first choice.

**`hangup` does not guess which line.** With one call running it takes that one,
and the tool needs no argument. With more than one it refuses — `error_code:
ambiguous_call`, and the result lists every line it could have meant, by number
and `call_id` — so the next call names one: `hangup {"call_id": "…"}`. Ending
the wrong line cannot be undone, and *the newest live call* was a coincidence
dressed as a rule. A `call_id` that names no running call is `no_call`, saying
which id it could not find. A caller waiting in the queue is not a running call
and `hangup` does not reach them; their leg ends when they hang up, or when they
are promoted and the call is ended the ordinary way.

**What holds the waiting hang-up bounded** is the media half's own promise: the
`voice` cell emits exactly one `speak_end` per `in_speak` it accepted — on the
last chunk, on a cancel, on a provider failure, and on a connection that went
away underneath it. There is no case in which it accepts a sentence and stays
silent. **What is NOT here is a clock** — see *What is not here*.

## Params

| knob | where | default | what it is |
|---|---|---|---|
| `emit_partials`, `stt`, `tts` | `voice` | see `templates/voice/README.md` | the media half |
| `mount` | `voice` | `phone` | ordered here: the door the media half answers on, on the colony's one listener. `phone` and not the template's own `voice`, so a member that also grows a browser voice channel keeps that name for it |
| `audio_out_frame_ms` | `voice` | `20` | ordered here: `mod_audio_stream` aborts the call on an outbound frame longer than about 100 ms |
| `emit_speak_end` | `voice` | `true` | ordered here: it is what *Hanging up* is built on. Off in the `voice` template itself |
| `fs_api_base_url` | `signal` | `${FREESWITCH_XMLRPC_BASE_URL}` | the switch's control endpoint, credentials included. The one provider lane |
| `voice_ws_url` **[operator-set]** | `signal` | `ws://127.0.0.1:7777/phone/ws` | where `mod_audio_stream` reaches the media half, *seen from the machine FreeSWITCH runs on*. Since `voice@2.0.0` the media half has no port of its own: the form is `ws://<listener>/<mount>/ws`, and `?session=<uuid>&sample_rate=<fork_sample_rate>` is appended. It is also the second field of the row `add_number` writes. The dialplan below captures `^ws://` and nothing else, so a `wss://` listener is refused there rather than dialled — it wants `https` for the API address and a second extension of its own |
| `db_realm` | `signal` | `meclaw_lines` | the realm of the switch's own table the three line tools write. Behind a shared switch a colony names the realm of the line it is reached on, `meclaw_lines_<dialled number>` — the number as it stands in `destination_number` on that switch, `+` included if the trunk delivers one — so `add_number`/`set_pin`/`disable_pin` write the realm the dialplan reads for that number (§ *What the dialplan owes*). The default is the one-number case |
| `line_user_id` **[operator-set]** | `signal` | `""` | the member a caller of this line is put through as: the first field of a number row and the key of the PIN row. Empty means the line tools write nothing and say so (`line_unconfigured`) |
| `dial_prefix` | `signal` | `sofia/gateway/fs02/` | what goes in front of the number in the dial string |
| `caller_id_number` | `signal` | `""` | the number this member calls from. Empty leaves it to the gateway |
| `answer_app` | `signal` | `&park()` | what the answered leg is handed to |
| `fork_sample_rate` | `signal` | `8000` | the stream's rate, and the rate the media half is asked to recognise at — the same number goes into `?sample_rate=` of the fork URL, so the two cannot disagree (GH #619). `8000` because a call already is 8 kHz. Written as a number because the `api_on_answer` value is one line at the switch; `"8k"` works too (`mod_audio_stream.c` v1.0.3 l. 170-177) and is read, not refused. A dialplan that also sets `STREAM_SAMPLE_RATE` sets it to this number: a single `?sample_rate=` answers both directions — binding inbound, a wish outbound — so the media half declares in `hello.audio_out.sample_rate` what actually comes back — with `?sample_rate=16000` that is 16000 whenever the synthesis provider serves it, and playing it back at another rate is the same audio read at the wrong speed |
| `ring_timeout_ms` | `signal` | `45000` | travels as `originate_timeout`. The clock is the switch's |
| `callers` **[operator-set]** | `signal` | `{}` | number → sender id |
| `second_call` | `signal` | `busy` | what an inbound call gets while the line is busy: `busy`, `queue` or `parallel` |
| `capacity` | `signal` | `1` | how many calls may run at once. Read for every policy, so `busy` is `parallel` with `1` |
| `queue_hold_media` | `signal` | `local_stream://moh` | what a waiting caller hears, as a `uuid_broadcast` source. A recorded announcement is a `file_string://…` |
| `external_timeout_ms` | `gateway` | `60000` | must exceed `ring_timeout_ms`: the originate answer arrives when the ringing stops |

**Since `2.0.2` three of them are declared `operator_set`** (GH #661). The three
marked above ship a value that is a SHAPE and not a working one: a URL pointing
at the machine the cell happens to run on, an empty identity, an empty allowlist.
A colony built with all three untouched has a telephone channel that reaches no
switch, and that is what was measured. A mutation that grows this hive without
setting them is refused at the door with `operator_param_unset`, naming all of
them at once, before anything is staged — so name them on the `add_nodes` entry
that grows the channel. Setting one to exactly the shipped value counts as
setting it; leaving it out does not. `dial_prefix` is deliberately not among
them: the gateway it names works.

## What is not here

**A dial plan.** Trunks, codecs, voicemail and inbound routing belong to
FreeSWITCH. What this template owes the dialplan is written down above and
nothing else.

**A sender for the media half's turns.** Spoken words carry the session and no
`user_id`, so a member with several callers needs the media half's turns to pass
through the signalling half, which would look each session's caller up in the
call table. One person per channel — the personal agent — is served by the
literal in the ingress edge, which is the shape `voice` already ships.

**Arbitration between two calls — RETRACTED in 1.1.0.** This paragraph used to
read: *A second call at a time. `hangup()` ends the newest live call, and the
table holds every call there is; nothing stops two, and nothing arbitrates
between them either. A channel that should serve a queue wants a lane that names
the call, and that is a wider question than a template.* The lane that names the
call is `hop.call_id`, and the arbitration is `params.second_call`
([#620](https://github.com/mmeyerlein/meclaw/issues/620)) — see *What a second
call gets*. What is **still** not here:

* **A queue deeper than the policy.** Every waiting caller sits in the table and
  the oldest is promoted, so a queue is as deep as the callers who ring; nothing
  caps it and nothing tells a caller their position.
* **A clock on a waiting caller.** Somebody in the queue waits until the call in
  front of them ends or until they hang up. The reason is the one the waiting
  hang-up gives below: a timeout would turn a wiring fault into a silence.
* **An airtight arrival count.** The decision reads the live calls and then
  writes the arriving row's state, so two legs that arrive inside one another's
  store round trip can both read the same count — the class the `speaking` count
  below already has, and the same cure it needs: an operation the store does not
  have.
* **A second call from the same number as a second conversation.** One
  conversation partner is one conversation, whichever spelling of
  `context.channel` is in force.
* **An order between the two commands of one bundle.** The pause and the hold
  media leave together, and so do the break and the resume. `gateway` is a
  `web_fetch`, which is a STATELESS cell: its dispatcher spawns one worker per
  message up to `params.max_concurrency` (4 here), so the two run at once and
  the switch may see either first. What that costs is milliseconds of a caller
  being heard by a recogniser that is about to be paused, or of silence before
  the hold media starts. Closing it would mean one command where FreeSWITCH
  offers two, or a `max_concurrency` of 1 on a cell that also carries the
  originate — neither is worth those milliseconds. What IS ordered is anything
  in two different bundles: the hold plays before the break that stops it,
  because a store round trip and a `call_ended` stand between them.
* **A handshake around the promotion.** The `resume` and the turn that announces
  the promoted caller leave in one bundle, and the switch carries out the first
  one when it gets to it. The media half's session was never closed — that is
  what `pause` buys — so an answer cannot arrive at a connection that is not
  there; what CAN happen is that the caller's first word is spoken into a stream
  that is still paused and is lost. Closing that needs a receipt from the switch
  that this template does not ask for.

**An airtight `speaking` count.** It is a count and not a flag, so two answers
in a row no longer let the first `speak_end` cut the second sentence off, and
the two read-backs above make the hang-up window symmetric — but the count
itself is kept by a read-modify-write over the store, so two `in_speak`s that
land inside one another's round trip can undercount. The window is one store
round trip wide and the worst it costs is the behaviour a flag had. Making it
exact needs an increment the store itself performs, and the store has no
arithmetic in `set`.

**A clock on the waiting hang-up.** A `hangup` that waits for `speak_end` waits
for as long as it takes, and the row records `hangup_at` so the wait is at least
*readable*. There is deliberately **no timeout** in this version, and the reason
is worth stating rather than hiding: the media half emits exactly one
`speak_end` per `in_speak` it accepted — including the failure and the
disconnect — so the only way the wait never ends is a **wiring** fault, an
`in_speak` that never reached the cell at all, and a timeout would turn that
fault into a silence instead of a bug report. The honest cure is a one-shot
`timer` per pending hang-up, removed when the `speak_end` arrives (never a
poll: `docs/cell-types.md` § `timer`, "what a timer is NOT for"), and it is not
built here. Until it is, a colony that wants the line cut regardless can send
the tool call again — a `hangup` with the flag already pending is answered by
the same wait, and the far end's own `call_ended` always ends the row.

## Versioning

`1.0.0` is the first shipped version **of this name**. It is a `1.0.0` and not a
`0.x` for the reason `docs/development-rules.md` § 4 gives: the lanes, the four
dialplan events and the two tool names are the surface a dialplan and a manifest
are written against, and a surface somebody wires against is a shipped fact.

Its one `ref` pins the `voice` template, and the pin itself stands in
`voice/config.json` — the one place a reader can resolve it, and the one place
§ 4a's sweep reads. `1.3.0` was the version that has the `speak_end` lane and
`1.4.0` is the one that reads `context.call_id`; an older `voice` cannot serve
this template, which is why the pin moves with it.

**`1.0.1` repairs the first real inbound call** ([#614](https://github.com/mmeyerlein/meclaw/issues/614)):
one turn per inbound call, no turn at the end of a call, and a `uuid_kill` on the
leg of a caller this channel refuses. It is a third-place bump because it moves
no lane, no tool name and no dialplan event: the four events, the two tools and
the five outbound lanes are the surface a dialplan and a manifest are written
against, and every one of them is untouched. What changed is what the channel
*says* on a lane that was already there, and
a colony that installed `1.0.0` needs no rewiring at all: the `swap_nodes` onto
`freeswitch@1.0.1` was the whole migration, and a `call_state` of `ended` simply
stopped appearing.

**`1.1.0` decides what a second call gets** ([#620](https://github.com/mmeyerlein/meclaw/issues/620)).
Second place, because a manifest can now declare something it never could:
`params.second_call` and `params.capacity` are new knobs, four receipt lanes are
new exits, and `hangup` takes a `call_id`. Both halves of the channel also stamp
`hop.call_id`, and the media half's pin moves with it, to the `voice` version
that reads `context.call_id` — the pin itself stands in `voice/config.json`.

Migrating a colony on `1.0.1` is `swap_nodes` onto `freeswitch@1.1.1` plus three
edits in the installing manifest: promote `call_id` instead of `voice_session`,
drop the `set_context` on the answer edge, and add the two receipt edges. The
first two are optional for one version, since the media half still reads
`context.session_id`; the third is not, and without it every inbound call
dead-letters one receipt.

**`1.1.1` moves the media half's pin** ([#639](https://github.com/mmeyerlein/meclaw/issues/639)).
Third place: no lane, no tool, no dialplan event and no param of this hive moves.
The `voice` cell it contains can now be reached under a mount name on the
colony's one listener, and the pin in `voice/config.json` moves with it. The
mount it asks for is `phone`, not the `voice` the template defaults to: a member
with a browser voice channel and a telephone therefore gets two mounts by
default, `voice` for the screen's half and `phone` for this one, and neither has
to be overridden for both to register. A colony on `1.1.0` migrates with
`swap_nodes` and nothing else.

**`2.0.0` makes the switch the proxy** ([#616](https://github.com/mmeyerlein/meclaw/issues/616)).
First place, because two things a caller wired against are different: the media
half answers on a **mount** and no longer on a port of its own
([#654](https://github.com/mmeyerlein/meclaw/issues/654)), and `call_incoming`
now carries an identity this hive **trusts** — `hop.user_id`, the
member the switch put the caller through as, with `params.callers` left as the
fallback for a dialplan that stamps none.

What moves with it: three tools beside `call` and `hangup` — `add_number`,
`set_pin`, `disable_pin` — which write the switch's own table and nothing here
(§ *What leaves for the switch*); two params on `./signal` (`db_realm`,
`line_user_id`); a `voice_ws_url` default that spells the new form
(`ws://127.0.0.1:7777/phone/ws`) and doubles as the second field of a row; and one
dialplan contract, written out as XML, that serves every line of every colony from
that one table (§ *What the dialplan owes*).

There is **no switchboard**. Which number belongs to which member, and what a
caller types before they are put through, are the proxy's business — this colony
holds no register of them and no PIN at all, and there is no tool that reads one
back.

Migrating a colony on `1.1.1`: `swap_nodes` onto `freeswitch@2.0.3`, then give
`./signal` a `line_user_id` (without it the three new tools refuse by name and
nothing else changes), and point `voice_ws_url` at the colony's listener and this
hive's mount instead of at a port. The dialplan keeps working unchanged as long
as it goes on stamping nothing: `callers` still answers who is on the line.

### The migration from `phone@1.0.0`

`phone@1.0.0` no longer exists: it was cut locally with 0.31.0 and never
exported, so for almost everybody this section is history. A colony that *did* grow the old node moves
in two steps and keeps its call table:

1. `swap_nodes` the node onto the new template
   (`{"match": {"name": "channels/phone"}, "template": "freeswitch@2.0.3"}`),
   which leaves the `store` where it is.
2. Rewrite the edges of the installing manifest above: they name the node, and
   the node's name is what changed. The receipt edges go in at the same time.

Renaming the *node* as well (`channels/phone` → `channels/freeswitch`) is the
tidier end state and costs the old node's `cell.db`: a call table is a log, not
a decision, so copy it or let it go.
