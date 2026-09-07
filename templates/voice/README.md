# `voice@1.3.0`

A spoken conversation as one cell. One WebSocket surface, one pair of provider
credentials, one wire up and one wire down. No persona, no memory, no answer of
its own -- it carries turns between somebody talking and whatever you put behind
it.

**The connection IS the session.** A client opens the socket, the cell mints a
`session_id` and every emission from that connection carries it. Closing the
socket ends the session; there is nothing to resume and nothing to garbage
collect. That is the one structural difference from a chat connector, where the
conversation outlives every connection, and it is why the key that routes an
answer back here is `session_id` and not a chat id.

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
| in | the finished assistant turn. `context.session_id` picks the connection it is spoken into |
| out, `hop.route == 'turn'` | one finished utterance as a user-origin text turn. `hop` carries `session_id`, `turn_id` (`<session_id>#<n>`), `platform` (`voice`) and `mode` |
| out, `hop.route == 'partial'` | an interim transcript, same body shape, `hop` carries `eager` beside the rest. OFF by default -- `params.emit_partials` turns it on |
| out, `hop.route == 'speak_end'` | one synthesis is over. Empty `messages[]`; `hop` carries `session_id`, `speak_id` and `reason` (`done`, `cancelled`, `failed`) beside `platform`. OFF by default -- `params.emit_speak_end` turns it on |
| out, `hop.route == 'error'` | the cell's own failure: empty `messages[]`, `hop.error_code` plus `msg_type: 'voice_error'`, `session_id` where one exists, detail in `meta` |

**Unlike a chat connector this cell names its own lanes.** `telegram-connector`
emits one wire and the level around it sorts the two shapes apart on
`has(hop.error_code)`; here the lane is already on `hop.route` when the emission
leaves, because there are four shapes and not two, and a level that had to
separate `partial` from `turn` by the presence of a key would be reading the
absence of `turn_id` as a meaning. So the edges below carry no `set_hop` on the
way up: the stamp the cell wrote is the stamp the container routes on.

## Wiring it into a member

**Adding a voice channel costs one node and two edges, and no template moves.**
The mutation is scoped to the **member**, not to the container: a node is
addressed by its `name` plus the scope, the name carries the `/`, endpoints are
scope-relative always, and scoping to `<member>/channels` would refuse `"to": "."`
with `edge_schema`.

```json
{"scope": "<member>", "diff": {
  "add_nodes": [{"name": "channels/voice", "template": "voice@1.3.0",
                 "override_params": {"port": 7900}}],
  "add_edges": [
    {"from": "./channels/voice", "to": "./channels",
     "condition": "has(hop.route) && (hop.route == 'turn' || hop.route == 'partial' || hop.route == 'error')",
     "modifier": {"set_context": {"channel_node": "'voice'",
                                  "channel": "'voice'",
                                  "assistant": "'<assistant>'",
                                  "audience_set": "'[\"agent:<assistant>\",\"member:<member>\"]'",
                                  "session_id": "has(hop.session_id) ? hop.session_id : ''",
                                  "voice_session": "has(hop.session_id) ? hop.session_id : ''"}}},
    {"from": "./channels", "to": "./channels/voice",
     "condition": "has(hop.route) && hop.route == 'answer' && has(context.channel_node) && context.channel_node == 'voice'",
     "modifier": {"set_hop": {"route": "'in_speak'"},
                  "set_context": {"session_id": "has(context.voice_session) && context.voice_session != '' ? context.voice_session : (has(context.session_id) ? context.session_id : '')"}}}
  ]
}}
```

**One edge up, not three.** All three outbound lanes carry the same promotion and
differ only in the stamp the cell already wrote, so splitting them into three
edges would buy nothing and give three copies of one modifier a chance to drift
apart. The container sorts them afterwards, on the same `hop.route` this edge
leaves untouched: the `member` container ships `./channels -> ./firewall` for
`turn` and `./channels -> .` for `error`. **`error` is promoted exactly like `turn`** -- a
failure that happened inside a session carries that session, and an edge that
promoted the session only on the happy path would make every failure look like it
came from nowhere.

**The promotion is not decoration.** `hop` is single-hop: it survives one
delivery. `session_id` has to be in `context` before the turn reaches anything
that will emit again, or the answer has no connection to be spoken into and the
cell answers `missing_session`. Every promotion off the hop is written
`has(...) ? ... : ''`, because a modifier that fails to evaluate skips the whole
edge -- and an edge that silently does not fire is the one failure mode a voice
surface diagnoses worst.

### `voice_session`, and why the connection needs a key of its own

The two extra lines in the manifest above look like belt and braces and are not.
`context.session_id` is the key the cell selects a connection by — and it is
**also** the key the member's `session-keeper` mints and stamps for its own
bookkeeping, on every turn that passes it. So on a member that holds a keeper,
the answer comes back carrying the keeper's generation id, no connection holds
it, and the cell answers `unknown_session` while the caller hears nothing. It
was found on a real telephone call (GH #603 § 3) and it is not specific to
telephony: it is what happens whenever a keeper stands between this cell and
whatever answers.

The workaround is the pair above: the ingress edge writes the connection's id
into `voice_session` as well, and the answer edge puts it back on `session_id`
on the way into the cell. It costs one context key and it is written down as a
**workaround** rather than a design — which of the two owns
`context.session_id` is a ruling nobody has made, and the day it is made these
two lines come out. A member with no `session-keeper` does not need them, and
they cost it nothing.

### The two channel keys, on a surface where they are the same word

`context.channel_node` is the **address** -- the node name in the container, which
every way back is guarded on. `context.channel` is the **conversation** -- what
the holders count by: `firewall` rate-limits one bucket per value,
`session-keeper` opens one generation per value, `memory-hive` writes it down as
the room a thing was said in (`templates/member/README.md` § *The two channel
keys*). A **screen** carries the same word in both because a screen is one room,
and **so does this cell**: one voice channel of a person is one room they speak
in, however many times they pick it up. The word is the node name.

`session_id` is the third key and it is neither of those two: it changes with
every connection, and the holders must not count by it or a person who redialled
would be a new room.

**That is why the promotion is on all three lanes and not only on `turn`.** A
phone edge (FreeSWITCH) passes its call UUID as `?session=` on the socket URL and
gets it back on every emission, which is how a phone hive matches turns to a
call. A `partial` or an `error` that arrived without the session would be an
event the bridge could not attribute to the call it belongs to -- and on a
failure that is exactly the moment attribution is worth the most.

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
                "session_id": "has(hop.session_id) ? hop.session_id : ''"}
```

A member whose firewall has no enabled `allow` row on `user_id` (the shipped
`member` template) needs no such line -- the dimension is unconstrained there.

### The `partial` lane ships OFF, and whoever listens orders it

`params.emit_partials` is `false` in this template. The cell then emits `turn`
only; interim transcripts still reach the client as frames on its own socket,
because that is where a live caption belongs -- they just do not become messages
on the topology.

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
{"name": "channels/voice", "template": "voice@1.3.0",
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
{"name": "channels/voice", "template": "voice@1.3.0",
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

`params.release_grace_ms` is `1500` here, and it is the difference between a
push-to-talk that keeps the end of your sentence and one that eats it. Letting
the key go says that no NEW audio belongs to this turn. It does **not** say the
turn is over: the recognition provider still owes the end of the audio it was
already sent, and it takes its time -- Deepgram Flux reports an end of turn
400-700 ms after the words that caused it. The boundary therefore stays open and
drains, `partial` frames keep arriving and still belong to the turn that is
closing, and exactly one `turn` leaves at whichever comes first: the provider's
own end of turn, or this cap, which cuts with the last interim.

That is a fix for two symptoms at once, both found on the built-in test page:
the last real line of a take went missing, and the take before it turned up at
the front of the next one -- the same late end of turn, first dropped and then
landing inside whatever boundary happened to be open when it arrived. Events
after the cut belong to no boundary now, so the next take starts empty, and a
key pressed again mid-drain closes the old take at once with what it has.

Set `0` to cut on the `release` frame, the behaviour this template had before
the knob existed. The value is read at the next `release`, so a params update
never moves a deadline a turn is already waiting on.

## Ports

`params.port` is owned exactly the way a `web` cell's is. **One instance per
port**: a second cell trying to bind the same one cannot start, and `0` is refused
at parse time rather than turned into "whatever the kernel hands out".

The default is `7900` and it is a template default, not an address: an instance
names its own through `override_params`, in the flat form, because a single-cell
template has nothing inside it to address.

```json
{"name": "channels/voice", "template": "voice@1.3.0",
 "override_params": {"port": 7912}}
```

`params.bind` defaults to `127.0.0.1` and wants to stay there. **This cell type
never authenticates and never will** -- the same rule the `web` cell carries.
Exposing a voice surface to a network means putting a proxy in front of it that
does the authentication and the TLS; it does not mean widening the bind. The key
is mutable at runtime because a rebind is a legitimate operation, not because
widening it is.

## The providers

Two traits, hosted adapters behind each and one that costs nothing, and which
one runs is `params.stt.provider` / `params.tts.provider`:

| | providers | the block carries |
|---|---|---|
| speech to text | `deepgram`, `openai`, `echo` | `api_key`, `model`, `language`, the end-of-turn thresholds, `sample_rate`, `base_url`; `deepgram` also `keyterms` |
| text to speech | `cartesia`, `openai`, `elevenlabs` | `api_key`, `voice`, `model`, `language`, `sample_rate`, `base_url` |

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
{"name": "channels/voice", "template": "voice@1.3.0",
 "override_params": {"stt": {"provider": "echo"}, "tts": null}}
```

That instance needs neither `.env` line. It is also why `requires.env` declares
both keys and requires neither -- see *The credentials* below.

**The `openai` adapters speak to any OpenAI-compatible endpoint.** `base_url` is
an ordinary config URL on both of them, so a local server implementing the same
routes -- a self-hosted realtime transcription endpoint, a self-hosted
`/v1/audio/speech` -- stands in for the hosted one without touching the cell:

```json
{"name": "channels/voice", "template": "voice@1.3.0",
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

**The cell never resamples.** The rate a provider declares is the rate on the
wire, in both directions, and audio arriving at another rate is refused rather
than converted. Deepgram takes 16 kHz, both OpenAI adapters and Cartesia are at 24
kHz by default, and so is `elevenlabs`; the client is what matches them, because
the client is the one place in the chain that knows what its microphone can do.

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
another. What IS on the update surface: `port` and `bind` (a rebind),
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
{"name": "channels/voice", "template": "voice@1.3.0",
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
{"name": "channels/voice", "template": "voice@1.3.0",
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
- **Not a screen.** It has no allowlist and no rate limit; anybody who reaches the
  port reaches your topology. Put [`firewall`](../firewall/) behind it -- shared
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
