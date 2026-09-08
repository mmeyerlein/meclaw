# The `meclaw-voice/1` wire protocol

What a client and a `voice` cell say to each other over one WebSocket. Audio
travels in binary frames, everything else as JSON text frames.

This is the transport spec. What the cell does with a turn, which lanes it emits
and which params it reads, is `cell-types.md` § `voice`. The two halves meet in
exactly one place: the `hello` frame, which declares the audio formats the
connection will use and which the client is expected to adapt to. The cell never
resamples.

## The connection

```
ws://<bind>:<port>/ws?session=<id>&mode=<auto|hold>&sample_rate=<hz>&encoding=<name>
```

All five are optional.

| Parameter | Meaning |
|---|---|
| `session` | The session identity, chosen by the client. At most 128 characters from `[A-Za-z0-9._:-]`. Reconnecting with the same `session` takes over the address: a second connection claiming a live `session` displaces the first, which is closed with `4409`. Without it the cell mints a uuid7. |
| `session_token` | An alias for `session`, same validation and same meaning, for edges that already carry a call identity under that name. `session` wins when both are set. |
| `mode` | `auto` or `hold`, overriding the cell's `default_mode` param for this connection. |
| `sample_rate` | The rate this client sends -- and would like to be sent. Without the parameter the providers' own declarations stand, which is the behaviour that predates it. For the inbound direction it is **binding**: the recognition session runs at it, or the connection is refused. For the outbound direction it is a wish (see below). |
| `encoding` | The sample encoding this client speaks. This version has exactly one, `pcm_s16le`; any other name is a `400` rather than an assumption. |

A phone edge such as FreeSWITCH passes its call UUID here, and every emission of
the connection then carries it as `hop.session_id`:

```
uuid_audio_stream <call-uuid> start ws://host:port/ws?session=<call-uuid>&sample_rate=8000 mono 8000
```

`mod_audio_stream` sends 8 kHz mono PCM16 that way -- the rate a telephone call
already is, and the two the module knows are `8k` and `16k` anyway. The number
stands **twice in the same line**, and that is the point: once for the module,
once for the cell. Keeping the command line and `hello.audio_in.sample_rate` in
agreement used to be a person's job; now the connection says the rate, and one
the recogniser does not serve is a `400` instead of a pitch nobody notices. The
freeswitch template writes both places out of one `fork_sample_rate`.

A `session` outside that shape, or a `mode` that is neither word, is refused
with `400` before the upgrade. Quietly replacing it would be worse: a client
that asked for an identity and got another one would address the wrong session
on every reconnect.

### The negotiated rate (GH #619)

A telephone call is 8 kHz. Upsampling it to 16 kHz before it reaches the
recogniser doubles the bytes and adds nothing: the bandwidth is what it was.
Deepgram Flux takes 8000 natively (its own quickstart lists
`8000, 16000, 24000, 44100, 48000` and calls `16000` the **recommendation**, not
a floor), ElevenLabs has `pcm_8000`, Cartesia serves `8000`.

The two directions are negotiated separately, and the asymmetry is deliberate:

- **Inbound is binding.** What a client *sends* has to be understood. If the
  recognition provider does not serve the rate, the connection is refused with
  `400` -- before the upgrade, like `session` and `mode`, and the answer names
  the rates it does serve. The cell does not resample (R-V2).
- **Outbound is a wish.** What a client *is sent* it can merely dislike. If the
  synthesis provider cannot do the rate, its own stands and `hello.audio_out`
  says so. One connection may therefore run at two rates.

`GET /info` names both sets (`audio_in_rates`, `audio_out_rates`), so a client
reads what it may ask for instead of provoking a refusal to find out.

This listener has no authentication and no TLS. It binds loopback by default;
anything else belongs behind a reverse proxy, the same stance the `web` cell
takes.

Three routes exist:

| Route | Answer |
|---|---|
| `GET /ws` | The WebSocket upgrade described here. A plain `GET` is a `400`. |
| `GET /` | The built-in browser test page (see below). |
| `GET /info` | The `hello` declaration as JSON, without a connection (see below). |

Anything else is a `404`.

## Audio frames

Every binary frame is audio, in both directions, in the format the `hello` frame
declared: raw PCM, signed 16-bit little-endian, mono, at the declared sample
rate. Chunk size is the sender's choice, and 20 to 80 ms per frame is the
recommendation.

The cell cuts the outbound frames, and the provider does not. `hello` declares
`audio_out_frame_ms`, 20 by default, and every binary frame the cell sends
carries at most that much audio: a synthesis chunk is split before it goes out,
so the number caps a frame instead of fixing its size. A frame is either exactly
that long or the remainder of a provider chunk, never longer; an even remainder
leaves at once instead of waiting for the next chunk, and the split never runs
through a sample. `0` means the cell sends each chunk exactly as its provider
produced it, and a client that cares about its worst-case frame size should read
the number instead of measuring it. The reason it exists is a telephony edge:
FreeSWITCH's `mod_audio_stream` 1.0.3 aborts the call on a frame longer than
about 100 ms.

Inbound and outbound rates are independent, because they belong to two different
providers: `audio_in` is the rate the speech-to-text provider demands,
`audio_out` the rate the text-to-speech provider produces. A client that can
only produce one rate resamples on its own side.

A binary frame whose length is not a whole number of sample frames, an odd byte
count for mono PCM16, is a protocol error. It is never trimmed. The frame is
dropped, the connection stays open, a per-connection counter goes up, and the
client is told with `error bad_audio_frame` carrying `bad_frames`, the number of
frames this connection has lost that way. The cell also reports it on its error
lane.

No threshold closes the socket. A client with a broken audio path gets a running
count it can act on. A wrong chunk size is a bug to fix, and hanging up on
somebody who is speaking would not fix it.

## Text frames the cell sends

Every text frame is a JSON object with a `type` field.

| `type` | Fields | When |
|---|---|---|
| `hello` | `protocol: "meclaw-voice/1"`, `session_id`, `call_id` (the same value under the name a channel addresses this connection by; since 1.4.0), `mode`, `audio_in {encoding, sample_rate, channels}`, `audio_out` (same shape, or `null` when no TTS provider is configured), `stt` (provider name), `tts` (provider name or `null`), `audio_out_frame_ms` (milliseconds of audio per outbound binary frame; `0` = the provider's own chunks, unframed), `speak_plain` (whether a written answer is turned into speech text before it is synthesised), `release_grace_ms` (how long a released `hold` boundary waits for the recognition provider's own end of turn before the cell cuts the turn; `0` = cut on the `release` frame) | Immediately after the upgrade, always the first frame on the connection. |
| `partial` | `text`, `eager: bool` | Every interim transcript. `eager` marks a preflight transcript: the provider thinks the turn is probably over but is not certain. |
| `turn` | `text`, `turn_id` | Exactly one per turn boundary. |
| `speak_start` | `speak_id` | Before the first audio frame of one synthesis. |
| `speak_end` | `speak_id`, `reason: "done" \| "cancelled" \| "failed"`, `detail?` | After the last audio frame of that synthesis, or when it was cut short. |
| `mode` | `mode` | A mode switch was accepted. |
| `error` | `code`, `detail`, `bad_frames?` | A protocol error on this connection. `bad_frames` is present on `bad_audio_frame` and counts what this connection has lost. |

`encoding` is always `"pcm_s16le"` and `channels` always `1` in this version.
The `sample_rate` of both formats is **this connection's** -- the provider's own
without `?sample_rate=`, the negotiated one with it, and the two directions may
differ.

What is spoken is not always what was written (`speak_plain`, `true` by
default). `hello` declares it because it changes what a client hears. With it on,
the cell turns the answer it was handed into speech text before synthesis:
markdown emphasis, heading hashes, list markers, link and image syntax, code
fences and table pipes go, a table row becomes its cells joined by commas, and a
line break becomes a sentence end. Prose with no markup in it is unchanged. With
`false` the provider is handed the answer exactly as it was written, stars
included. The declared value is the one the cell's I/O half was built with: a
runtime `params` update takes effect on the next synthesis and reaches this
declaration on the next respawn.

### Error codes

The list is closed. Treat a code outside it as a bug, never as an extension.

| `code` | Meaning |
|---|---|
| `bad_frame` | A text frame was not valid JSON, or not one of the client frames below. The connection stays up. |
| `bad_audio_frame` | A binary frame was not whole sample frames. It was dropped; the connection stays up, and `bad_frames` says how many this connection has lost. |
| `wrong_mode` | A frame that only exists in one mode arrived in the other (`hold`/`release` in `auto`, or any of them against the echo provider). |
| `not_holding` | `release` without an open `hold`. |
| `already_holding` | `hold` while a `hold` is already open, or a `mode` switch during one. |
| `stt_failed` | The speech-to-text session of this connection failed. Sent on the **first** failure as well as the last: the cell re-establishes the session once, a second later, and audio sent in that window reaches no provider and is dropped. A second failure closes the connection with `1011`. |
| `tts_failed` | A synthesis failed. The matching `speak_end` carries `reason: "failed"`. |

## Text frames the client sends

| `type` | Fields | Meaning |
|---|---|---|
| `hold` | (none) | Open the turn boundary. `hold` mode only; in `auto` it answers `wrong_mode`, twice in a row `already_holding`. A running synthesis is cancelled, because pressing the button is barge-in. |
| `release` | (none) | Close the turn boundary: no more audio belongs to this turn. Exactly one `turn` frame follows, as soon as the recognition provider reports the end of the audio already sent, and at the latest after `release_grace_ms` (default 1500 ms; `0` answers on the frame itself). On an empty hold it carries `text: ""`. |
| `cancel` | (none) | Drop the running synthesis and this session's queue. Answered with `speak_end … "cancelled"`. |
| `mode` | `mode` | Switch this connection between `auto` and `hold`. Refused with `already_holding` while a `hold` is open; while one is draining it is accepted and closes that boundary first, so exactly one `turn` still comes out of it. |

Audio needs no frame of its own: a binary frame is audio, always.

## The two modes

In `auto` the provider decides where a turn ends. Interim transcripts arrive as
`partial`, the boundary as exactly one `turn`. A provider that withdraws its own
end-of-turn does not un-send that `turn`; the continuation becomes the next one.
Speech starting while the cell is speaking cancels the synthesis when `barge_in`
is on.

In `hold` the client decides. Audio flows to the provider continuously, so the
recognition session stays warm, but only what happens between `hold` and
`release` counts: provider end-of-turns append to a buffer, `partial` frames
show buffer plus current interim, and `release` produces exactly one `turn` from
the whole thing.

`release` does not deliver the `turn`. It says that no new audio belongs to this
turn; the provider still owes the end of what it was already sent, and it takes
400 to 700 ms over it (Deepgram Flux). So the boundary drains. `partial` frames
keep coming and still belong to the turn that is closing, and the `turn` frame
follows at whichever comes first, the provider's end-of-turn or the cap
`release_grace_ms`, which then cuts with the last interim. Anything the provider
says after that belongs to no boundary, so the next `hold` starts empty. A
`hold` sent while one is still draining closes that one at once, with what it
has, and opens the new one; a `mode` frame mid-drain does the same closing (a
boundary that has been released cannot be released again, so refusing it would
be a dead end for as long as the grace runs).

Whenever something other than the provider closes a boundary, meaning the key
again, the cap or a mode switch, the session remembers a provider end it is
still owed. The provider is still inside the turn the old audio started, and its
next end-of-turn carries the take that has just closed. That one event pays the
debt and is thrown away, whether it lands inside the next boundary or outside
every boundary, so pressing the key again early and pressing it late are the
same. The debt is written off if the recognition session dies first.

`release_grace_ms` is declared in `hello` and by `GET /info`, so a client knows
the upper bound it is waiting on instead of guessing one.

In both modes the invariant is the same: one `turn` per boundary, and never a
partial on the turn lane.

## Close codes

| Code | Meaning |
|---|---|
| `4409` | A second connection claimed this `session`; this one is the older. |
| `1011` | The speech-to-text session ended twice and could not be kept up. A provider that closes its session cleanly counts the same way: after the one retry, an ended session is an ended connection, because a socket nothing is listening on is worse than one that says so. |

Ordinary closes (`1000`, `1001`) mean what they mean everywhere else.

## The echo provider

With `stt.provider: "echo"` the cell is a loopback: every binary frame comes
back byte-identical, in order, and no JSON is sent except `hello`, which
declares `stt: "echo"`, `tts: null`, `audio_out` equal to `audio_in` and
`audio_out_frame_ms: 0`, because a loopback that reframed would not be one.
`speak_plain` is declared there too and means nothing, since an echo cell speaks
no text at all. `hold`, `release` and `cancel` are answered with
`error wrong_mode`.

The point is calibration. It measures the socket, the client's audio path and
the round trip without a model in the way, so a slow first turn can be blamed on
the right half.

## An example session (`hold` mode)

```
C->S  GET /ws?session=demo&mode=hold        (upgrade)
S->C  {"type":"hello","protocol":"meclaw-voice/1","session_id":"demo",
       "mode":"hold","audio_in":{"encoding":"pcm_s16le","sample_rate":16000,"channels":1},
       "audio_out":{"encoding":"pcm_s16le","sample_rate":24000,"channels":1},
       "stt":"deepgram","tts":"cartesia","audio_out_frame_ms":20,
       "speak_plain":true,"release_grace_ms":1500}
C->S  {"type":"hold"}
C->S  <binary>  320 bytes of PCM16 @ 16 kHz, 10 ms      (repeated while the key is down)
S->C  {"type":"partial","text":"what is the","eager":false}
C->S  {"type":"release"}                                 (no more audio for this turn)
S->C  {"type":"partial","text":"what is the weather","eager":false}   (still this turn)
S->C  {"type":"turn","text":"what is the weather","turn_id":"demo#1"}  (provider end, or the cap)
S->C  {"type":"speak_start","speak_id":"demo#1s"}
S->C  <binary>  PCM16 @ 24 kHz                          (repeated)
S->C  {"type":"speak_end","speak_id":"demo#1s","reason":"done"}
```

The `turn` frame is a mirror: the same text left the cell as a message into the
colony at that moment, and the answer that comes back as `speak_start` … audio
… `speak_end` is what the colony sent back.

## `GET /info`

The `hello` declaration without opening a connection. A client can ask what it
would be told before it commits to a session, and an operator can read the
wiring with `curl`.

```json
{
  "protocol": "meclaw-voice/1",
  "mode": "auto",
  "audio_in": {"encoding": "pcm_s16le", "sample_rate": 16000, "channels": 1},
  "audio_out": {"encoding": "pcm_s16le", "sample_rate": 24000, "channels": 1},
  "audio_in_rates": [8000, 16000, 24000, 44100, 48000],
  "audio_out_rates": [8000, 16000, 22050, 24000, 44100, 48000],
  "stt": "deepgram",
  "tts": "cartesia",
  "audio_out_frame_ms": 20,
  "speak_plain": true,
  "release_grace_ms": 1500
}
```

`mode` is the cell's configured default and never a connection's mode, and there
is no `session_id`, because nothing was opened. `audio_in`/`audio_out` are what
a client that asks for nothing gets; `audio_in_rates`/`audio_out_rates` are what
it could ask for. `audio_out` and `tts` are `null`
when no text-to-speech provider is configured, `audio_out_rates` too, and
`audio_out_frame_ms` is `0` there for the same reason. `speak_plain` is reported either way, because it
describes the cell itself and a single synthesis does not change it.

## Built-in test page

`GET /` serves a self-contained browser test page on the same listener as the
socket, with no build step, no CDN and no files on disk. Open it, choose `hold`
or `auto`, press connect, and the `hello` frame's audio formats and provider
names appear above the log.

Holding the button, or the space bar, opens the microphone through an inline
`AudioWorklet`, resamples it to the declared `audio_in` rate and sends 20 ms
PCM16 LE frames, bracketed by `hold` and `release` in hold mode. `partial`,
`turn`, `speak_start`, `speak_end` and `error` are logged with timestamps
relative to the connection, returned audio plays back gaplessly, and a `cancel`
button cuts a synthesis off.

The microphone needs a secure context. Browsers hand out `getUserMedia` only on
`https://` or on `localhost`, so the page works when it is reached as
`http://localhost:<port>/`, over an SSH tunnel for instance, or through a TLS
proxy in front of the cell. A LAN IP over plain `http` will load the page and
then fail to get a microphone. That is a browser rule, and the cell cannot grant
an exception to it.

Everything else on the page works without a microphone: the connection, the
`hello` declaration, the frame log and the audio playback.

## Reserved, not built

Two frame types are named here and left unimplemented, so that a later
speech-to-speech session can be composed into the same protocol instead of
beside it:

- `spoken`: the model's own transcript of what it said.
- `tool_call`: a tool call the speech model made inside its session.

The lanes of the same names are reserved for that composition. A client must
ignore text frames whose `type` it does not know, and this version sends
neither.
