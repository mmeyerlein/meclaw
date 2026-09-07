# `freeswitch@1.0.1`

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

* the **media half** is a `voice` cell, unchanged: it binds a WebSocket port,
  takes audio in and gives audio back, and puts ordinary text turns on the
  topology.
* the **signalling half** is a small state machine — one `code` cell that offers
  the tools, one that keeps the book, one `web_fetch` cell that talks to the
  switch and one `store` that holds the calls.

They live in one hive because a call is one thing, and because **one id binds
both halves**: FreeSWITCH's channel UUID is the `?session=` of the audio fork,
and therefore the `session_id` an answer is spoken back into. A colony that kept
the two apart would have to invent a second table to say which socket belongs to
which number.

**FreeSWITCH stays the media edge, in gateway mode.** It does the SIP, it owns
the trunk, and `mod_audio_stream` connects to the media half as a WebSocket
*client*. Nothing in this template speaks SIP, and nothing in it holds a trunk.

## The cells

| path | type | from |
|---|---|---|
| the template root itself | `hive`, `ports: []` | the address. No port; the two connect points are the one exception this template pronounces about itself |
| `voice` | `ref` | the `voice` template — the media half, with `audio_out_frame_ms: 20` and `emit_speak_end: true` ordered here |
| `dial` | `code` | the offer: `call` and `hangup`, and the menu entry that makes them callable |
| `signal` | `code` | the book: every lane becomes a store bundle, and the store's answer becomes a turn, a command at the switch, or a tool result |
| `gateway` | `web_fetch` | the one command channel to `mod_xml_rpc` |
| `calls` | `store` | the calls this channel has going |

## What travels

| direction | what travels |
|---|---|
| in, `in_speak` | the finished assistant turn, to be spoken into the call. `context.session_id` picks the connection, exactly as it does for a `voice` channel standing on its own |
| in, `call_incoming` | somebody is ringing this member. `hop` carries `call_uuid` and `number`. This is THE turn of an inbound call — a number in no `callers` entry gets no turn and its leg is put down |
| in, `call_ringing` | the switch is ringing a number this channel dialled. It moves the row and raises no turn |
| in, `call_answered` | somebody picked up. `hop` carries `call_uuid`. A turn for a call this channel PLACED; for an inbound call it moves the row and raises no turn, because `call_incoming` already said it |
| in, `call_ended` | the line is down, from either end. `hop` carries `call_uuid` and `cause`. It moves the row and raises no turn |
| in, `tool` / `schemas` | a call to `call`/`hangup`, and the menu tick. Both dock on `<freeswitch>/dial` |
| out, `hop.route == 'turn'` | one thing that was said, or one thing that happened to the line. `hop` carries `session_id`, `turn_id` (`<session_id>#<n>`), `platform` (`phone`), `number`, `user_id`, and `call_state` (`incoming`, `answered`, `busy`, `no_answer`, `failed`) for a turn about the line rather than about words |
| out, `hop.route == 'partial'` | an interim transcript, out of the media half. OFF by default — `emit_partials` on the `voice` cell turns it on |
| out, `hop.route == 'error'` | a caller with no entry in `callers` — whose leg is put down in the same breath — a request this hive cannot read, or the media half's own failure |
| out, `tool_result` / `tool_schemas` | the receipt of a `call`/`hangup`, and the offer itself |

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
  `call_answered` behind it moves the row and says nothing;
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

## The two tools

```
call(number, purpose)   ring somebody up. `number` in international form,
                        `purpose` one short line about why.
hangup()                end the call that is running. If the assistant is in
                        the middle of a sentence, the line stays up until that
                        sentence is finished -- see *Hanging up* below.
```

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

`params.callers` on `./signal` maps **number → sender id**:

```json
{"callers": {"+493012345678": "alex", "+491701234567": "robin"}}
```

The id becomes `hop.user_id` on every turn the signalling half raises, and the
member's firewall allowlists senders by exactly that key. **A number with no
entry gets no id**, and an incoming call from it is refused with
`unknown_caller` — one `error` out of the rim, and no turn. That is deliberate:
a turn without a sender is a turn a member that allowlists one person refuses
anyway, and a refusal that says *which* number rang is worth more than a rejected
turn.

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
  "add_nodes": [{"name": "channels/freeswitch", "template": "freeswitch@1.0.1",
                 "override_params": {
                   "voice": {"port": 7910, "bind": "0.0.0.0"},
                   "signal": {"dial_prefix": "sofia/gateway/fs02/",
                              "voice_ws_url": "ws://<colony-host>:7910/",
                              "caller_id_number": "<the number this member calls from>",
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
                                  "session_id": "has(hop.session_id) ? hop.session_id : ''",
                                  "voice_session": "has(hop.session_id) ? hop.session_id : ''"}}},
    {"from": "./channels", "to": "./channels/freeswitch",
     "condition": "has(hop.route) && hop.route == 'answer' && has(context.channel_node) && context.channel_node == 'freeswitch'",
     "modifier": {"set_hop": {"route": "'in_speak'"},
                  "set_context": {"session_id": "has(context.voice_session) && context.voice_session != '' ? context.voice_session : (has(context.session_id) ? context.session_id : '')"}}},

    {"from": "./channels/freeswitch", "to": "./channels",
     "condition": "has(hop.route) && (hop.route == 'tool_result' || hop.route == 'tool_schemas')",
     "modifier": {"set_context": {"tool_answerer": "'freeswitch'",
                                  "session_id": "has(hop.session_id) ? hop.session_id : (has(context.session_id) ? context.session_id : '')"}}},
    {"from": "./assistants/<gen>/talky", "to": "./channels/freeswitch/dial", "lane": "tool",
     "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && (hop.tool_name == 'call' || hop.tool_name == 'hangup')",
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

Read it in three groups.

**The channel half** is the first two edges, and they are the `voice` channel's
own two, word for word (`templates/voice/README.md` § *Wiring it into a member*):
one promotion up, one restamp down. `session_id` is promoted on **all three**
outbound lanes and not only on `turn`, because a failure that happened inside a
call carries that call, and an edge that promoted the session only on the happy
path would make every failure look like it came from nowhere. `user_id` is the
one line the `voice` channel writes as a bare literal and this one writes as a
fallback: the signalling half knows the caller, the media half does not.

**`voice_session` is the fourth key, and it exists because somebody else owns
the third** (GH #603, defect 3). The media half selects the connection it speaks
into by `context.session_id`. Between the channel and the assistant stands the
member's `session-keeper`, whose whole job is to mint and stamp a session of its
OWN on that same key — so the answer came back carrying the keeper's generation
id, no connection held it, and the cell answered `unknown_session` while the
caller listened to silence. The manifest works around it in two lines: the
ingress edge writes the call's id into `voice_session` as well, and the answer
edge puts it back on `session_id` on the way into the channel. It is a
**workaround and it is written down as one**: which of the two owns
`context.session_id` is a ruling nobody has made, and the day it is made this
pair of lines comes out.

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
| `call_incoming` | an inbound leg arrives, before it is answered | one turn *“Incoming call from …”* — or, for a number in no `callers` entry, one `unknown_caller` error **and** a `uuid_kill` on that leg |
| `call_ringing` | an outbound leg starts ringing | moves the row, raises nothing |
| `call_answered` | either leg is answered — the same place the audio stream is started | for a call this channel PLACED: one turn *“… answered. Purpose of this call: …”*. For an INBOUND one: moves the row, raises nothing — `call_incoming` was the turn |
| `call_ended` | the leg hangs up | moves the row, `state` and `cause`, and raises nothing |

`call_uuid` is the **same** UUID that hangs on the stream URL as `?session=`. For an
outbound call the channel mints it and hands it to the switch as
`origination_uuid`, so both ends of the wire already agree before the telephone
rings.

**If meclaw does not answer, the dialplan falls back to the mailbox.** That
belongs in the dialplan and not here — a `curl --max-time` and a condition on its
exit code; a person who rings a colony that is down should hear a mailbox, not
silence. The **refusal** of a known-good colony is the case that exit code does
not cover — the `curl` succeeds and carries a refusal — which is why this hive
kills that leg itself rather than leaving it parked (see *Who is on the line*).

## What leaves for the switch

Three commands, all as a GET at `mod_xml_rpc`'s `/webapi` endpoint — `web_fetch`
implements GET and nothing else (`docs/cell-types.md` § `web_fetch`), and
`/webapi/<command>?<args>` is exactly a GET. The endpoint, its credentials and
its host live in **one** provider lane, `${FREESWITCH_XMLRPC_BASE_URL}`, which is
the only `${…}` token in this template:

```
GET <base>/webapi/originate?{origination_uuid=<uuid>,originate_timeout=45,
      ignore_early_media=true,
      api_on_answer='uuid_audio_stream <uuid> start <voice_ws_url>?session=<uuid> mono 16000'}
      <dial_prefix><number> &park()
GET <base>/webapi/uuid_kill?<uuid>
GET <base>/webapi/uuid_break?<uuid>%20all
```

**`api_on_answer`, not `execute_on_answer`, and `uuid_audio_stream`, not
`uuid_audio_fork`** (GH #603, defect 2). Starting the stream is an **API
command**, and `execute_on_answer` runs an **application**: FreeSWITCH answered
`Invalid Application` and hung the freshly answered call up, on the first real
call this template ever made. Two more things about that one line:

- **the rate is written as `16000`.** Both spellings reach the module —
  `mod_audio_stream.c` v1.0.3 lines 170-177 read `16k`/`8k` by `strcmp` and
  everything else through `atoi` — and the number is what this template writes,
  because the whole `api_on_answer` value is one line at the switch and a digit
  string is the form that cannot be mistaken for a unit. `fork_sample_rate`
  defaults to `16000`, and an instance carrying `"16k"` is read rather than
  refused.
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

**What holds the waiting hang-up bounded** is the media half's own promise: the
`voice` cell emits exactly one `speak_end` per `in_speak` it accepted — on the
last chunk, on a cancel, on a provider failure, and on a connection that went
away underneath it. There is no case in which it accepts a sentence and stays
silent. **What is NOT here is a clock** — see *What is not here*.

## Params

| knob | where | default | what it is |
|---|---|---|---|
| `port`, `bind`, `emit_partials`, `stt`, `tts` | `voice` | see `templates/voice/README.md` | the media half |
| `audio_out_frame_ms` | `voice` | `20` | ordered here: `mod_audio_stream` aborts the call on an outbound frame longer than about 100 ms |
| `emit_speak_end` | `voice` | `true` | ordered here: it is what *Hanging up* is built on. Off in the `voice` template itself |
| `fs_api_base_url` | `signal` | `${FREESWITCH_XMLRPC_BASE_URL}` | the switch's control endpoint, credentials included. The one provider lane |
| `voice_ws_url` | `signal` | `ws://127.0.0.1:7900/` | where `mod_audio_stream` reaches the media half, *seen from the machine FreeSWITCH runs on* |
| `dial_prefix` | `signal` | `sofia/gateway/fs02/` | what goes in front of the number in the dial string |
| `caller_id_number` | `signal` | `""` | the number this member calls from. Empty leaves it to the gateway |
| `answer_app` | `signal` | `&park()` | what the answered leg is handed to |
| `fork_sample_rate` | `signal` | `16000` | the stream's rate. Has to match the media half's STT rate. Written as a number because the `api_on_answer` value is one line at the switch; `"16k"` works too (`mod_audio_stream.c` v1.0.3 l. 170-177) and is read, not refused |
| `ring_timeout_ms` | `signal` | `45000` | travels as `originate_timeout`. The clock is the switch's |
| `callers` | `signal` | `{}` | number → sender id |
| `external_timeout_ms` | `gateway` | `60000` | must exceed `ring_timeout_ms`: the originate answer arrives when the ringing stops |

## What is not here

**A dial plan.** Trunks, codecs, voicemail and inbound routing belong to
FreeSWITCH. What this template owes the dialplan is written down above and
nothing else.

**A sender for the media half's turns.** Spoken words carry the session and no
`user_id`, so a member with several callers needs the media half's turns to pass
through the signalling half, which would look each session's caller up in the
call table. One person per channel — the personal agent — is served by the
literal in the ingress edge, which is the shape `voice` already ships.

**A second call at a time.** `hangup()` ends the newest live call, and the table
holds every call there is; nothing stops two, and nothing arbitrates between
them either. A channel that should serve a queue wants a lane that names the
call, and that is a wider question than a template.

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
§ 4a's sweep reads. `1.3.0` is the version that has the `speak_end` lane; an
older `voice` cannot serve this template, which is why the pin moved with it.

**`1.0.1` repairs the first real inbound call** ([#614](https://github.com/mmeyerlein/meclaw/issues/614)):
one turn per inbound call, no turn at the end of a call, and a `uuid_kill` on the
leg of a caller this channel refuses. It is a third-place bump because it moves
no lane, no tool name and no dialplan event: the four events, the two tools and
the five outbound lanes are the surface a dialplan and a manifest are written
against, and every one of them is untouched. What changed is what the channel
*says* on a lane that was already there, and
a colony that installed `1.0.0` needs no rewiring at all: the `swap_nodes` onto
`freeswitch@1.0.1` is the whole migration, and a `call_state` of `ended` simply
stops appearing.

### The migration from `phone@1.0.0`

`phone@1.0.0` no longer exists: it was cut locally with 0.31.0 and never
exported, so for almost everybody this section is history. A colony that *did* grow the old node moves
in two steps and keeps its call table:

1. `swap_nodes` the node onto the new template
   (`{"match": {"name": "channels/phone"}, "template": "freeswitch@1.0.1"}`),
   which leaves the `store` where it is.
2. Rewrite the five edges of the installing manifest above: they name the node,
   and the node's name is what changed. The two new keys go in at the same time
   — `voice_session` on the way up, the `session_id` restore on the way down.

Renaming the *node* as well (`channels/phone` → `channels/freeswitch`) is the
tidier end state and costs the old node's `cell.db`: a call table is a log, not
a decision, so copy it or let it go.
