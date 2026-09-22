# `voice@2.2.0`

A spoken conversation as one cell. One WebSocket surface, one pair of provider
credentials, one wire up and one wire down. No persona, no memory, no answer of
its own -- it carries turns between somebody talking and whatever you put behind
it.

**The connection IS the session.** A client opens the socket, the cell mints an
id and every emission from that connection carries it. Closing the socket ends
the session; there is nothing to resume and nothing to garbage collect. That is
the one structural difference from a chat connector, where the conversation
outlives every connection, and it is why the key that routes an answer back here
is the connection's own and not a chat id.

**Since 1.4.0 that id travels under two names, and the second one is the one to
wire against.** Every emission carries it as `hop.session_id` and as
`hop.call_id`, and an answer selects its connection with `context.call_id`. One
value, two names, because the first name has a second owner one level up: a
member's `session-keeper` mints and stamps `context.session_id` for its own
generation on every turn that passes it (GH #603 § 3). `call_id` only ever means
the connection. `context.session_id` is still read where `call_id` is absent, so
a colony wired against `voice@1.3.0` keeps working unchanged.

## The cell

| path | type | from |
|---|---|---|
| the template root itself | `voice` | nothing else -- this template is one cell |

Nothing sits below it. The node an edge names IS the cell -- wherever the
instantiating mutation put it -- and not a scope with a door, so there is no
`hive_port_boundary` to trip over and no lane name to hit.

## What travels

| direction | what travels |
|---|---|
| in | the finished assistant turn. `context.call_id` picks the connection it is spoken into (`context.session_id` where that key is absent) |
| in, `hop.route == 'in_advise'` | one section of advice appended to a RUNNING duplex session, without cutting anybody off: `hop.section` says whether it is a `fact` (said out loud), a `context` (thought) or a `correction` (the standing instruction rewritten). Duplex only -- a cascade session has no channel to append to and answers `wrong_engine` |
| out, `hop.route == 'turn'` | one finished utterance as a user-origin text turn. `hop` carries `session_id` and `call_id`, `turn_id` (`<session_id>#<n>`), `platform` (`voice`) and `mode` |
| out, `hop.route == 'partial'` | an interim transcript, same body shape, `hop` carries `eager` beside the rest. OFF by default -- `params.emit_partials` turns it on |
| out, `hop.route == 'spoken'` | what the ASSISTANT is saying while it is still saying it, same body shape, `hop` carries `speaker: 'assistant'`. Duplex only, and on the same `params.emit_partials` -- the two halves of one stream are ordered together or not at all |
| out, `hop.route == 'speak_end'` | one synthesis is over. Empty `messages[]`; `hop` carries `session_id`, `call_id`, `speak_id` and `reason` (`done`, `cancelled`, `failed`) beside `platform`. OFF by default -- `params.emit_speak_end` turns it on |
| out, `hop.route == 'delegation'` | the duplex provider handed work to the client and named the handle: `hop` carries `delegation_id` and `offset_ms`, the body the user text of the open turn. Duplex only |
| out, `hop.route == 'error'` | the cell's own failure: empty `messages[]`, `hop.error_code` plus `msg_type: 'voice_error'`, `session_id` and `call_id` where a session exists, detail in `meta` |

**A duplex emission stamps `hop.engine: 'duplex'`; a cascade stamps nothing at
all.** The absent key is the older behaviour, so a colony wired before the
duplex provider existed sees exactly the hops it always saw, and an edge that
has to tell the two engines apart reads `has(hop.engine)` rather than a word
that would have to be invented for the cascade.

**Unlike a chat connector this cell names its own lanes.** `telegram-connector`
emits one wire and the level around it sorts the two shapes apart on
`has(hop.error_code)`; here the lane is already on `hop.route` when the emission
leaves, because there are six shapes and not two, and a level that had to
separate `partial` from `turn` by the presence of a key would be reading the
absence of `turn_id` as a meaning. So the edges below carry no `set_hop` on the
way up: the stamp the cell wrote is the stamp the container routes on.

## Wiring it into a member

**Adding a voice channel costs one node and two edges, and no template moves.**
(A duplex instance adds one more edge, for the advice lane -- see *The
providers* below.)
The mutation is scoped to the **member**, not to the container: a node is
addressed by its `name` plus the scope, the name carries the `/`, endpoints are
scope-relative always, and scoping to `<member>/channels` would refuse `"to": "."`
with `edge_schema`.

```json
{"scope": "<member>", "diff": {
  "add_nodes": [{"name": "channels/voice", "template": "voice@2.2.0",
                 "override_params": {"mount": "voice"}}],
  "add_edges": [
    {"from": "./channels/voice", "to": "./channels",
     "condition": "has(hop.route) && (hop.route == 'turn' || hop.route == 'partial' || hop.route == 'spoken' || hop.route == 'error' || hop.route == 'delegation')",
     "modifier": {"set_context": {"channel_node": "'voice'",
                                  "channel": "'voice'",
                                  "assistant": "'<assistant>'",
                                  "audience_set": "'[\"agent:<assistant>\",\"member:<member>\"]'",
                                  "call_id": "has(hop.call_id) ? hop.call_id : ''",
                                  "session_id": "has(hop.session_id) ? hop.session_id : ''"}}},
    {"from": "./channels", "to": "./channels/voice",
     "condition": "has(hop.route) && hop.route == 'answer' && has(context.channel_node) && context.channel_node == 'voice'",
     "modifier": {"set_hop": {"route": "'in_speak'"}}}
  ]
}}
```

**One edge up, not five.** Every outbound lane carries the same promotion and
they differ only in the stamp the cell already wrote, so splitting them into one
edge each would buy nothing and give five copies of one modifier a chance to
drift apart. The container sorts them afterwards, on the same `hop.route` this
edge leaves untouched: the `member` container ships `./channels -> ./firewall`
for `turn`, `./channels -> ./assistants` for `delegation` and `./channels -> .`
for `error`. The two duplex lanes are in the condition even on a cascade
instance, where nothing ever travels them: an edge that has to be widened before
a `params.duplex` can be switched on is a second migration for a knob. **`error` is promoted exactly like `turn`** -- a
failure that happened inside a session carries that session, and an edge that
promoted the session only on the happy path would make every failure look like it
came from nowhere.

**The promotion is not decoration.** `hop` is single-hop: it survives one
delivery. `call_id` has to be in `context` before the turn reaches anything
that will emit again, or the answer has no connection to be spoken into and the
cell answers `missing_session`. The way back needs no modifier at all: nothing
between the channel and the assistant writes `context.call_id`, so it arrives on
the answer exactly as it left. Every promotion off the hop is written
`has(...) ? ... : ''`, because a modifier that fails to evaluate skips the whole
edge -- and an edge that silently does not fire is the one failure mode a voice
surface diagnoses worst.

### `voice_session` is gone, and what replaced it

**Retracted in 1.4.0.** Until 1.3.0 the manifest above carried two extra lines,
and this section described them as a workaround waiting for a ruling:

> `context.session_id` is the key the cell selects a connection by -- and it is
> **also** the key the member's `session-keeper` mints and stamps for its own
> bookkeeping, on every turn that passes it. So on a member that holds a keeper,
> the answer comes back carrying the keeper's generation id, no connection holds
> it, and the cell answers `unknown_session` while the caller hears nothing.

The diagnosis was right and it was found on a real telephone call (GH #603 § 3).
The two lines wrote the connection's id into a second context key,
`voice_session`, and put it back on `session_id` on the way in.

The ruling has been made (GH #620): **the call key belongs to the channel.** The
cell now stamps `hop.call_id` on everything it emits and selects a connection by
`context.call_id`, `context.session_id` stays the keeper's, and the two lines
come out of the manifest. Nothing is restamped on the way down, because nothing
on the way down overwrites a key nobody else owns.

**A colony on the old wiring does not have to move on the same day.**
`context.session_id` is still read where `call_id` is absent, so the pair of
lines keeps working until the manifest is rewritten; the cell prefers `call_id`
whenever both are there, which is exactly what a half-migrated colony needs.

### The two channel keys, on a surface where they are the same word

`context.channel_node` is the **address** -- the node name in the container, which
every way back is guarded on. `context.channel` is the **conversation** -- what
the holders count by: `firewall` rate-limits one bucket per value,
`session-keeper` opens one generation per value, `memory-hive` writes it down as
the room a thing was said in (`templates/member/README.md` § *The two channel
keys*). A **screen** carries the same word in both because a screen is one room,
and **so does this cell**: one voice channel of a person is one room they speak
in, however many times they pick it up. The word is the node name.

`call_id` is the third key and it is neither of those two: it changes with
every connection, and the holders must not count by it or a person who redialled
would be a new room.

**That is why the promotion is on all three lanes and not only on `turn`.** A
phone edge (FreeSWITCH) passes its call UUID as `?session=` on the socket URL and
gets it back on every emission as `hop.call_id`, which is how a phone hive
matches turns to a call. A `partial` or an `error` that arrived without the call
would be an event the bridge could not attribute to the call it belongs to -- and
on a failure that is exactly the moment attribution is worth the most.

**The round comes from this edge.** `audience_set` has exactly one spelling and
no template may introduce a second (`member`, GH #330). Nothing upstream of
a channel knows who is in the room, so the channel's own ingress edge is the
setter root: it is a JSON list, written as a CEL string literal, naming the agent
that answers and the person who is speaking. A turn that arrives without it is
refused by the holders on the `reject` lane the member already drains -- with a
reason, which is what makes promoting an empty string the safe half of the trade.

**The sender, where the member allowlists senders.** A member grown from a seed
that closes its firewall on one person -- an enabled `allow` row on `user_id`,
the shape a personal agent ships with -- refuses every turn that arrives without
a promoted `context.user_id` as `sender_not_allowed`, and a voice turn carries
no chat identity of its own. So the ingress edge of such a member promotes the
one speaker as a literal, beside the round (measured 2026-09-05 on a colony
grown from a production seed: every voice turn rejected until this line stood):

```json
"set_context": {"channel_node": "'voice'", "channel": "'voice'",
                "assistant": "'<assistant>'",
                "audience_set": "'[\"agent:<assistant>\",\"member:<member>\"]'",
                "user_id": "'<the person's sender id>'",
                "call_id": "has(hop.call_id) ? hop.call_id : ''"}
```

A member whose firewall has no enabled `allow` row on `user_id` (the shipped
`member` template) needs no such line -- the dimension is unconstrained there.

### The `partial` lane ships OFF, and whoever listens orders it

`params.emit_partials` is `false` in this template. The cell then emits `turn`
only; interim transcripts still reach the client as frames on its own socket,
because that is where a live caption belongs -- they just do not become messages
on the topology.

On the topology side nothing is dropped: a listener that falls behind stalls
the sender, and every interim waits for it. On the socket side the wait ends by
count: a client that leaves 64 queued commands untaken loses its connection, and
the cell reports that on the `error` lane as `client_too_slow`, with
`hop.dropped_frames` naming how many went with it.

**The reason is the exit, and there is none by default.** `partial` would reach
`./channels` and stop there: the `member` container ships no edge for it, so
every interim would dead-letter with `no_route` -- once per interim, forever, in
the queue a colony reads when something is actually wrong. A lane nobody ordered
that fills the dead-letter queue is not a feature waiting to be used; it is
noise that trains an operator to stop reading the DLQ.

**So the lane is ordered by whoever consumes it, in the same breath as the
edge that carries it.** The reader of a running transcript is an **app**, and
the `./channels -> ./apps` rim is its own piece of work (later: `meclaw app
install`). Until then the manifest that wants partials does both halves itself
-- turns the lane on and draws the edge that drains it -- and the first half is
one key on the node:

```json
{"name": "channels/voice", "template": "voice@2.2.0",
 "override_params": {"emit_partials": true}}
```

Both halves or neither. Turning the lane on without an edge is the one
configuration this template asks you not to ship: it costs no turn -- the
finished `turn` is unaffected -- and it buries the queue.

### The `speak_end` lane ships OFF as well, and the telephone is who orders it

`params.emit_speak_end` is `false` in this template, and it is off for the
`partial` lane's reason rather than for one of its own: the emission goes to the
cell's own path, the out-edges decide where it lands, and a lane nobody drew an
edge for dead-letters once per sentence.

**What it is.** One emission per `in_speak` the cell accepted, when that
synthesis is over -- whichever way it ended. `hop.reason` says which: `done`
(the last chunk went out), `cancelled` (a `cancel`, a barge-in or a `hold` cut
it short) and `failed` (the provider gave up). It carries no words: whoever is
waiting for the sentence to finish already had the sentence.

**Who needs it.** A colony that has to do something *after* the assistant has
finished speaking, and that has no other way to know when that is. The
[`freeswitch`](../freeswitch/) channel is the case this lane was built for, in
two places: a `hangup` while the assistant is still talking waits for the
`speak_end` of that call before it kills the leg, and a `cancelled` or `failed`
one is the signal to stop what the far edge is still playing (`uuid_break
<uuid> all`), because `mod_audio_stream` dispatches only
start/stop/pause/resume/send_text and offers no clear of its own (verification
at the switch pending — see [`freeswitch`](../freeswitch/) § *Hanging up*).

**Both halves or neither**, exactly as for `partial`:

```json
{"name": "channels/voice", "template": "voice@2.2.0",
 "override_params": {"emit_speak_end": true}}
```

...and the edge that drains the lane in the same manifest. The client's own
`speak_end` frame on the socket is a separate path and travels either way -- a
browser page has always been told when the speaking stopped.

### Outbound audio leaves in 20 ms frames

`params.audio_out_frame_ms` is `20` here. A synthesis provider picks its own
chunk size -- Cartesia's are large -- and the cell cuts every chunk into frames
of at most this length before it writes them to the socket. It is an upper
bound, not a fixed size: a frame is either exactly that long or the remainder of
a provider chunk, never longer, and an even remainder leaves at once rather than
waiting for the next chunk. The cut never runs through a sample, and a
part-sample tail travels with the next chunk instead of being dropped, so the
bytes and their order are exactly what the provider produced.

**FreeSWITCH `mod_audio_stream` needs frames of 100 ms or less; the shipped 20 ms
default is safe.** Measured on the real socket with 1.0.3: 960 B (20 ms), 4410 B
and 4800 B (100 ms) play, while 9600 B (200 ms) and up abort the process with
`free(): corrupted unsorted chunks` -- a `SIGABRT` in its closed-source playback
half, which takes the call with it. Nothing is paced or delayed: a burst of small
frames is what that module expects, and a gap of 100 ms is what it cannot take.

Set `0` to send each chunk exactly as the provider produced it -- the behaviour
this template had before the knob existed, and the right value only for a client
whose playback path has no frame-size limit at all.

### Markdown never reaches the microphone

`params.speak_plain` is `true` here. An assistant writes for a screen whether or
not anybody asked it to -- `**emphasis**`, `# headings`, `- lists`,
`[links](https://example.com)`, code fences, table pipes -- and a synthesis provider reads what
it is handed. The first live call of this cell had Cartesia pronouncing the
stars around a bolded word, which is not a provider bug: it was given markdown
and asked to speak it.

So the cell rewrites the answer into speech text before it enters the session's
queue -- the queue holds the text that will be synthesised, and rewriting after
an answer was queued would let a later `params` flip reach answers that were
already accepted. The markup goes, the words stay: a link keeps its text and an
image its alt text, a code fence loses its fence and keeps its code, a heading
loses its hashes, a blockquote its `>`, a list item its marker and its `[ ]`
box, an escape its backslash but not the character behind it, a table row becomes its
cells joined by commas, and a line break becomes a sentence end (a full stop
unless the line already ends in `.`, `!`, `?`, `:`, `;` or `,`). Nothing else is
touched -- umlauts, punctuation and digits travel as they are, HTML entities and
tags are left alone, and an answer with no markup in it comes out byte for byte
as it went in.

Two shapes stay because somebody dictated them: a `*` or `_` with whitespace on
both sides is an arithmetic operator (`3 * 4`), and an ordinal at the start of a
line keeps its digit and loses only the punctuation -- `5. September 2026`
becomes `5 September 2026`, because a date and a list item look identical there
and a lost day costs more than an ungainly list. And an answer with no words
left in it -- a rule, an empty emphasis, an assistant turn that arrived empty --
is neither spoken nor refused: nothing is queued, and the cell says so at debug
level.

Set `false` where the text must reach the provider exactly as it was written --
the behaviour this template had before the knob existed. The flag is on the
runtime update surface and takes effect on the next answer; the `hello` frame
and `GET /info` DECLARE it, and that declaration follows on the next respawn,
because the reading is the handler's work and the declaration is the I/O half's.

### `release` waits for the provider, it does not cut

`params.release_grace_ms` is `2500` here, and it is the difference between a
push-to-talk that keeps the end of your sentence and one that eats it. Letting
the key go says that no NEW audio belongs to this turn. It does **not** say the
turn is over: the recognition provider still owes the end of the audio it was
already sent, and it takes its time. The boundary therefore stays open and
drains, `partial` frames keep arriving and still belong to the turn that is
closing, and exactly one `turn` leaves at whichever comes first: the provider's
own end of turn, or this cap, which cuts with the last interim.

**The grace counts from the release, but it has to pay for the time since the
last WORD.** That is the number this default is set by, and it is not the one a
model quotes. An endpointing provider waits for a stretch of silence in the
audio stream and spends its model latency only after that threshold has fired,
so Deepgram Flux's 400-700 ms is the second half alone. Measured end to end on
a live colony (18.09.2026, three machine-timed holds against one fixture): the
`EndOfTurn` arrives **1795 ms after the last spoken word**. Since 2.0.4 the cap
is 2500 ms for exactly that reason -- at 1500 every hold released by hand was
cut on the cap with the last interim, and the end of the sentence was missing.
The only take that survived was held on to for 1.24 s after the last word,
which is a person doing by hand what this number is for.

**Draining is not waiting in silence.** Since 2.0.3 the cell pushes 20 ms frames
of digital silence into the recognition session from the `release` on, at the
rate this connection negotiated, until the provider ends the turn or the grace
runs out. Waiting alone was not enough: an endpointing provider measures the end
of a turn in the AUDIO STREAM -- Flux after `eot_threshold`, 0.7 s -- and the
client stops sending about 120 ms after the key comes up, so nothing ever
reached the threshold and every held take was cut at the cap with an interim
that lags the audio. Measured on a fresh instance with a fixture released 20 ms
after the last word: cut at +1501 ms, transcript empty; the same take with
silence still streaming closed 137 ms after the release with the whole sentence.
The client's own frames outrank the tail -- while it is still draining its
capture graph the next silence frame waits -- so no zeroes land between the last
two words. `release_grace_ms: 0` arms no tail: it cuts on the frame.

That is a fix for two symptoms at once, both found on the built-in test page:
the last real line of a take went missing, and the take before it turned up at
the front of the next one -- the same late end of turn, first dropped and then
landing inside whatever boundary happened to be open when it arrived. Events
after the cut belong to no boundary now, so the next take starts empty, and a
key pressed again mid-drain closes the old take at once with what it has.

Since 2.0.2 the cap records a debt only while the provider is actually inside
a take: a provider that delivered its end of turn before the key came up owes
nothing, and a debt it never owed used to be paid by the end of the NEXT take,
which then never became a turn. An end of turn without a transcript keeps the
interim instead of dropping it.

Set `0` to cut on the `release` frame, the behaviour this template had before
the knob existed. The value is read at the next `release`, so a params update
never moves a deadline a turn is already waiting on.

### Since 2.0.1 the recognition session lives per hold

In `hold` mode the session opens with the first `hold` rather than with the
connection, and it ends when the provider's own idle deadline
(`params.provider_idle_timeout_ms`, `30000`) expires with the key up. That
ending is silent: no `error` frame, no `stt_failed` on the error lane, no close.
The next `hold` opens a new session, and the audio behind the frame waits in the
queue while it comes up.

It is the arrangement a held key already implies. Between two holds a
push-to-talk page sends nothing, so the deadline was certain to expire, and what
the client was told was that its microphone had failed -- twice, and then a
`1011` that closed the socket under a page nobody had touched yet. `auto` mode
is unchanged: there the session is the call, and a provider that stops
listening ends it.

## The door

An instance is reached at `/<mount>/` on the colony's one listener. That is the
whole of it: `params.mount` is required, and `port` and `bind` are gone.

`params.mount` ships as `voice`, which puts the socket at `/voice/ws` and the
declaration at `/voice/info` on whatever address the colony's `--api` listener
holds. The grammar is `[a-z0-9-]{1,64}`, and the segments the API owns
(`colony`, `messages`, `health`, `ui`, `live`, `@client`) are refused. **Two
instances need two mounts**: the second one to register under a name another
cell holds registers nothing and says so in the journal. The key is mutable, and
a new name takes effect on the next life of the cell — the registration happens
once, when the I/O half starts.

```json
{"name": "channels/voice", "template": "voice@2.2.0",
 "override_params": {"mount": "voice-b"}}
```

**Migrating from `1.x`:** drop `port` and `bind` and name a mount. A params
document that still carries either is refused at parse time with the sentence
that says so, rather than starting a cell nobody can reach at the address that
was written down.

**This cell type never authenticates and never will** -- the same rule the
`web` cell carries. Exposing a voice surface to a network means putting a proxy
in front of the colony's listener that does the authentication and the TLS.

## The providers

Two traits, hosted adapters behind each and one that costs nothing, and which
one runs is `params.stt.provider` / `params.tts.provider`. Since 2.1.0 there is
a third trait beside them that replaces both at once -- see *The third path*
below -- and a cell runs either the pair or the single provider, never a mix:

| | providers | the block carries |
|---|---|---|
| speech to text | `deepgram`, `openai`, `echo` | `api_key`, `model`, `language`, the end-of-turn thresholds, `sample_rate`, `base_url`; `deepgram` also `keyterms` |
| text to speech | `cartesia`, `openai`, `elevenlabs` | `api_key`, `voice`, `model`, `language`, `sample_rate`, `base_url` |

And what each one will run at, which is what a client negotiates against
(GH #619). Every adapter here speaks `pcm_s16le` mono and nothing else:

| provider | direction | default rate | rates it serves |
|---|---|---|---|
| `deepgram` | in | `16000` | `8000`, `16000`, `24000`, `44100`, `48000` |
| `openai` (transcription) | in | `24000` | `24000` |
| `echo` | both | `16000` | `8000`, `16000`, `24000`, `44100`, `48000` -- the union of the recognisers above, so a calibration can be run at the rate the measurement after it will use |
| `cartesia` | out | `24000` | `8000`, `16000`, `22050`, `24000`, `44100`, `48000` |
| `elevenlabs` | out | `24000` | `8000`, `16000`, `22050`, `24000`, `32000`, `44100`, `48000` |
| `openai` (speech) | out | `24000` | `24000` |

The fields are the provider's own, so they are not all the same: `elevenlabs`
takes `stability` and `similarity_boost` instead of `language`, `emotion` and
`speed`, and its `sample_rate` accepts only the rates the vendor serves
(`8000`, `16000`, `22050`, `24000`, `32000`, `44100`, `48000`). A key the
provider has no wire for is refused as unknown rather than ignored.

**`deepgram` takes `keyterms`.** A list of words the recogniser should expect --
`"keyterms": ["Sam", "meclaw"]` inside the `stt` block -- because a general model
hears a name it was never trained on as the nearest name it knows. Each entry is
boosted on its own and a multi-word entry stays one term; the list is empty by
default. It lives in the `stt` block, so like the rest of that block it is set at
instantiation and not on the runtime params surface.

**`echo` is the provider that needs no credential.** It transcribes nothing and
hands the audio straight back, so a `voice` cell with `stt.provider: "echo"` and
**no `tts` block at all** is a complete, spendable-nothing proof that the socket,
the framing and the wiring work. It is the first thing to instantiate when a
colony grows its first voice channel, and the last thing to fall back to when a
provider is the suspect.

**Getting there needs `"tts": null`, and the reason is a property of overrides.**
`override_params` MERGES onto what the template ships; it has no gesture that
REMOVES a key. So overriding `stt` onto `echo` and saying nothing about `tts`
leaves the shipped Cartesia block standing, credential and all. `null` is the
spelling that says "not set" -- `VoiceParams::parse` reads a null `tts` exactly
as an absent one, which is legal precisely when the recogniser is `echo`:

```json
{"name": "channels/voice", "template": "voice@2.2.0",
 "override_params": {"stt": {"provider": "echo"}, "tts": null}}
```

That instance needs neither `.env` line. It is also why `requires.env` declares
both keys and requires neither -- see *The credentials* below.

**The `openai` adapters speak to any OpenAI-compatible endpoint.** `base_url` is
an ordinary config URL on both of them, so a local server implementing the same
routes -- a self-hosted realtime transcription endpoint, a self-hosted
`/v1/audio/speech` -- stands in for the hosted one without touching the cell:

```json
{"name": "channels/voice", "template": "voice@2.2.0",
 "override_params": {
   "tts": {"provider": "openai",
           "base_url": "http://<local-host>:<port>",
           "api_key": "${LOCAL_TTS_KEY}",
           "model": "<the model that server serves>",
           "voice": "<the voice that server serves>"}}}
```

The credential still travels as a `${VAR}`, even where the endpoint ignores it: an
empty value is the explicit statement "this endpoint needs no credential"
(`docs/config.md` § *The empty value*), and a literal in a mutation body ships a
secret into the `mutation_log`.

**The cell never resamples.** The rate on the wire is the rate a provider is
actually running at, in both directions, and audio arriving at another rate is
refused rather than converted. The client is what matches it, because the client
is the one place in the chain that knows what its microphone can do.

**But the client may say which rate that is** (GH #619). The query rides the
one URL — `ws://<listener>/<mount>/ws?session=<id>&sample_rate=8000` — and it
tells the cell what this connection sends; the recognition session then runs at that rate, and `hello` declares the
pair it agreed to. The two directions are answered separately, because they are
not the same kind of promise: **inbound is binding** -- a rate the recogniser
does not serve refuses the connection with a `400` that names the rates it does
-- while **outbound is a wish**: a rate the synthesis provider cannot do leaves
its own rate standing, and `hello.audio_out` says which. `GET /info` lists both
sets as `audio_in_rates` and `audio_out_rates`, so a client reads what it may
ask for rather than provoking a refusal to find out. A client that asks for
nothing gets exactly what it got before: the defaults in the table above.

The case this exists for is the telephone. A call IS 8 kHz; upsampling it to
16 kHz at the switch doubled the bytes and added no bandwidth, and the
recogniser was handed interpolated samples for its trouble. The `freeswitch`
template now streams 8 kHz and asks for 8 kHz in the same command line, out of
one `fork_sample_rate`.

### The third path: one provider that hears and speaks

**`params.duplex` replaces the pair with a single socket.** A duplex provider
takes audio and gives audio back on one connection, so there is no recogniser
handing text to a synthesiser and no seam between them to tune. Two ship:

| provider | what it is | the block carries |
|---|---|---|
| `gpt_live` | a hosted live model | `api_key`, `base_url`, `model`, `voice`, `sample_rate`, `instructions`, `greeting`, `turn_gap_ms`, `backchannel_max_ms`, `spoken_quiet_ms`, `spoken_cap_ms`, `close_grace_ms`, `tick_ms`, `keepalive_ms`, `delegation_grace_ms`, `delegation_fallback` |
| `echo` | the loopback, which needs no credential and knows only `sample_rate` | -- |

**`echo` is here for the reason `echo` is always here.** A trait with one
implementation is a shape borrowed from that implementation (ADR-0023), and a
loopback that hands every frame straight back proves the socket, the framing and
the wiring without spending anything.

**One format, both directions.** A duplex session negotiates ONE rate and one
encoding for what it hears and what it says, and `16000` or `24000` are the two
`gpt_live` serves -- `8000` is refused by the vendor, and this cell never
resamples, here as everywhere else.

**The block is exclusive with `stt` and `tts`.** A params document that names
`duplex` and either of the other two is refused, so switching an instance over
sets both to `null` in the same breath -- `override_params` merges and has no
gesture that removes a key:

```json
{"name": "channels/voice", "template": "voice@2.2.0",
 "override_params": {"duplex": {"provider": "gpt_live",
                                "api_key": "${OPENAI_API_KEY}",
                                "instructions": "<who the model is for this session>",
                                "sample_rate": 24000},
                     "stt": null, "tts": null}}
```

`instructions` is required and not empty: it is who the model is for this
session, and it cannot be changed while the session runs. `greeting` is said
once after the session opens, so the model speaks first; empty means it waits
for the caller. Both live in the `duplex` block, which carries a credential and
is therefore off the runtime params surface exactly like `stt` and `tts`:
rotation means `.env` plus a restart, and tuning means a respawn.

**A running session can still be told something, and that is the `in_advise`
lane.** It is the one thing a duplex session offers that a cascade cannot: a
fact, a piece of context or a correction appended to the session while it runs,
without cutting the speaker off and without becoming a turn. It needs one edge
down beside the answer edge:

```json
{"from": "./channels", "to": "./channels/voice",
 "condition": "has(hop.route) && hop.route == 'in_advise' && has(context.channel_node) && context.channel_node == 'voice'"}
```

The lane arrives already stamped -- the level that sorts an assistant's sections
is what names it -- so the edge carries no `set_hop` at all. The words of the
section travel in `body.payload` -- the splitter upstream cuts a `sidecar` block
by top-level key and puts the section's own VALUE there, not `{section: value}`
-- and that slot is read first, because `hop.section` is what named it.
`messages[-1].text` and `body.text` are read after it, for a sender that writes
one of those instead. Where the payload is an object rather than a sentence, the
words are found by name: the section's own key, then `text`, then the first
string in sorted key order. A section that carries no words in any
of the three is refused with `bad_section` rather than dropped -- an advice that
disappears is indistinguishable, from the outside, from a model that ignored it.
On a cascade instance the same message is refused with `wrong_engine`, which is
the honest answer: there is no open channel to append to.

**The session has a clock of its own, and it is a timeout timer.** Turns here
are cut on the MODEL's timeline, so a caller who stops talking closes a turn
only when time passes -- and on a line where nobody talks, no fragment arrives
to say that it did. `tick_ms` (`1000`) is the interval that says it: the
connection hands the turn machine the time on every tick, and the same tick
carries the delegation deadline below. It replaces the provider's running meter,
`session.usage.updated`, which was doing the job until 2.2.0 and does not hold
it -- on a session that hears only silence the meter arrived at no point inside
45 s over four measured runs, which is exactly the line it was relied on for.
The meter is now read for `usage_ratio` and for nothing else. A watchdog timer
is not polling; it is a timeout timer, and the event-driven rule is untouched.
The promise above -- a turn closes between one and two gaps after the caller
fell quiet -- holds while `tick_ms` is at most `turn_gap_ms`; a raster coarser
than the gap it measures waits longer than the gap says. A `tick_ms` of zero is
refused at the parse, because a clock with no period is a busy loop.

**And the session asks its own socket whether it is still there.** Every
`keepalive_ms` (`8000`) the adapter sends a WebSocket ping, and the pong that
comes back carrying ITS payload -- and only that one -- resets
`provider_idle_timeout_ms`. Without it that deadline could not tell a caller who
paused from a wire that had died: session frames alone do not say which of the
two a silence is, so thirty seconds of silence on the line read exactly like
thirty seconds of dead socket, and the call was cut. A ping somebody sends US
still counts for nothing -- a proxy pinging a dead upstream would otherwise hold
a call open for as long as it likes -- and the asymmetry is the point: our own
question, our own answer.

Eight seconds, and not a round third of the shipped thirty. Pings leave at 8, 16
and 24 s, and the keepalive arm sits below the idle arm, so a ping due exactly
on the deadline loses. The third one therefore has to be the one that saves the
call, and at 24 s it is back with six seconds to spare; at ten seconds it would
have left AT the deadline, and only one lost pong would have been survivable.
Eight is also inside the sixty seconds a proxy commonly allows an idle socket.
Keep any value you set here a fraction of `provider_idle_timeout_ms` -- a
keepalive as long as the deadline never resets it, and the spawn says so in a
warning.

**A delegation nobody answers is closed by the cell.** One left open longer than
`delegation_grace_ms` (`12000`) gets a single `Commentary` append on its own
`delegation_id` carrying `delegation_fallback` (`"Das kann ich gerade nicht
nachsehen."`), and then it leaves the open list -- the sentence falls once, not
once per tick. Measured, an unanswered delegation leaves the caller with one
holding sentence and then 55 to 58 s of silence, and no timeout arrives from the
provider's side inside a minute. The shipped grace is about twice the slowest
answer the same measurements saw end to end, 6 000 ms to reach the cell plus
779 ms for the model to acknowledge the append; six observations are not a
distribution, so the number leaves room rather than shaving it. The first line
against this case is still the prompt -- an assistant that answers every
delegation, empty-handed if need be -- and the fallback is an instruction rather
than a script, because a live model paraphrases what it is given.

An answer that arrives AFTER the fallback is still appended. The delegation is
closed on this side and gone from the open list, and the append that carries the
late answer names a `delegation_id` the model has already seen closed; what the
model does with it is not measured, and in the ordinary case it is simply a
second thing said into the same conversation. So the caller may hear the
fallback and then the answer, in that order, which is the trade the grace was
chosen for.

**When the provider is gone, so is the call.** A duplex session is not
reconnected: the connection IS the session here as everywhere in this cell, and
a provider that drops the socket ends the call rather than resuming it
somewhere the caller cannot hear.

## The credentials

`params.stt.api_key` and `params.tts.api_key` are `${DEEPGRAM_API_KEY}` and
`${CARTESIA_API_KEY}`, substituted once, at instantiation, out of the colony's
`.env`. After that the `config.json` is a bootstrap imprint and nothing rewrites
it: rotating a key means editing `.env` and restarting.

**Neither token carries a default, and that is on purpose.** A colony that grows
the shipped configuration without both lines in its `.env` does not start: it
stops with `env_var_missing` naming the variable, which is the loud failure a
token without a default exists for. `template.json` declares both under
`requires.env` so a builder reads the environment surface instead of discovering
it -- declared, but not `required`, and the second half is the load-bearing one:
`stt.provider` is a param, an instantiation may override it onto `echo`, and the
requirement walk reads the TEMPLATE and never the `override_params` beside it.
Requiring either key here would therefore refuse the one configuration that is
meant to cost nothing.

**Neither provider block is on the runtime update surface at all**, and that is
a stronger statement than immutability. A `proxy`'s `bot_token` IS a key of its
update surface and is refused there by name, as `Immutable`; `stt` and `tts` are
not in this cell's `KNOWN_KEYS`, so a params update naming one is refused as
`unknown param 'stt'` -- the refusal a key that does not exist gets, rather than
the refusal a key that exists and may not move gets. The reason is the same
either way: the block is the cell's identity at a third party, and swapping it
under a live connection would leave half a turn spoken in one voice and half in
another. What IS on the update surface: `mount` (read on the next life),
`default_mode`, `barge_in`, `emit_partials`, `emit_speak_end`, `audio_out_frame_ms`,
`speak_plain`, `release_grace_ms` and the two timeouts.

**`voice` is a knob, not a credential.** A voice id says which timbre speaks,
which is behaviour, so it lives in `params` where `override_params` reaches it
rather than in a colony-wide environment line (`docs/development-rules.md` § 8a,
R6 -- a `${VAR}` inside a shipped template is refused by
`scripts/check_tree_rules.py`). The same goes for `model`, `language` and the
end-of-turn thresholds: every one of them is a param with a default, and none of
them is a constant in the code.

**The shipped id is Cartesia's own example voice, and it really speaks.** It is
`a0e99841-438c-4a64-b679-ae501e7d6091`, the voice id Cartesia's own request
examples carry beside `model_id: "sonic-3.6"` and, in the reference's own
generation example, `sonic-latest` --
[the TTS reference](https://docs.cartesia.ai/api-reference/tts/tts) and
[the continuations guide](https://docs.cartesia.ai/build-with-cartesia/capability-guides/stream-inputs-using-continuations)
both print it. That is the whole point of picking it: a first smoke with no
override at all produces speech rather than a provider error, so the first thing
that fails is the wiring and not the credential-shaped guess in the config. It is
still a **placeholder** -- an English-timbre voice next to `language: "de"` -- and
every real instance names its own.

**Wanting the voice in `.env` after all is one line, and it goes in the
MANIFEST.** R6 binds the shipped template, not the mutation that instantiates it:
an operator who keeps voice ids beside the credentials writes the token into the
instantiating manifest's `override_params`, where it is substituted at
instantiation exactly like the two api keys.

```json
{"name": "channels/voice", "template": "voice@2.2.0",
 "override_params": {"tts": {"provider": "cartesia",
                             "api_key": "${CARTESIA_API_KEY}",
                             "voice": "${CARTESIA_VOICE}"}}}
```

`tts` is one block on the params surface, so an override of it replaces the whole
block -- name the provider and the credential beside the voice, or the defaults
of the keys you dropped are the ones that apply.

**Another provider is the same one block.** `elevenlabs` takes its voice id in
exactly the same place, and the whole switch is one override -- the template does
not change, because `provider` was always a value rather than a shape:

```json
{"name": "channels/voice", "template": "voice@2.2.0",
 "override_params": {"tts": {"provider": "elevenlabs",
                             "api_key": "${ELEVENLABS_API_KEY}",
                             "voice": "${ELEVENLABS_VOICE}"}}}
```

The voice id is a **path segment** of that vendor's endpoint rather than a
request field, so an unsubstituted `${…}` there would not be a wrong voice, it
would be a wrong URL -- which is why the parser refuses it by name, along with
any value carrying `/`, `?`, `#`, `&`, `%` or whitespace. And there is no cancel
message in that protocol: a barge-in closes the socket, which the adapter does
and the vendor accepts as the end of the generation.

## What it is not

- **Not a phone.** There is no SIP, no PSTN and no room: a client brings audio
  over the WebSocket and takes audio back. Whatever bridges a telephone to that
  socket stands outside this cell.
- **Not a screen.** It has no allowlist and no rate limit; anybody who reaches
  the mount reaches your topology. Put [`firewall`](../firewall/) behind it -- shared
  across channels, so an attacker touching three of them is one pattern and not
  three thirds.
- **Not a bot.** This is the wire, and the agent behind it answers. A complete
  spoken bot is this cell plus something that produces answers -- see the
  [`talky`](../talky/) template.
- **Not a session store.** It keeps one connection's state in the connection and
  nothing after it closes. A conversation that has to survive a hang-up is what
  `session-keeper` is for, one level up.
- **Not a level.** It normalises nothing on anybody else's behalf; the level that
  holds it decides what its three lanes mean, and that level is `channels`.
