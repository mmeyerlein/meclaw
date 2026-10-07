# Stability

Five surfaces are the public contract of this project: the HTTP API, the template DSL, the
template ports, the mount a surface cell owns, and the documented `error_code` strings. If you
build on one of them, a `0.x` release will not take it away from you without saying so. If you
build on anything else in the tree, you are building on an internal.

## The five surfaces

The HTTP API means the `/colony/*` routes and `POST /messages`. They are specified in
[`meclaw-overview.md`](meclaw-overview.md).

The template DSL is the `template.json` and `config.json` schemas, including the mutation diff
format. [`config.md`](config.md) documents the `config.json` schema, and the overview the diff.

The template ports are the ingress and exit endpoints a template's README declares. Each
template's README names the ports that template has.

The mount a surface cell owns is `/<mount>/` on the colony's listener. Two cell types own one. For
a `web` cell it is the mount with its `page.set` route grammar and the two reserved names, `@` and
`live` ([`cell-types.md`](cell-types.md) § `web`). For a `proxy` cell on `platform: "meclaw"` it
is the peer mount another colony posts to, and the wire-v1 frame that crosses it: the message
frame and the receipt frame with their keys, the header that names the sending colony and the
addresses it is believed from (`params.trusted_proxies`, loopback by default), the
credential the outgoing POST carries (`params.auth`: a static header, or an OAuth 2.0
client-credentials bearer), the origins that POST may go to (`params.egress`, nothing without
it), and the eleven `error_code` strings a refusal carries
([`cell-types.md`](cell-types.md) § `proxy`). The peer mount is the surface a colony you do not run
builds against, and the credential is what the proxy in front of it checks.

The documented `error_code` strings are the dead-letter reasons, the cell-type codes, and the
codes a `/colony` read can answer with. The overview lists them in the sections that emit them.

## What 0.x promises

Changes to those five are additive. A route, a key, a port or a code that is there today is there
tomorrow, and new ones arrive beside it.

A change that breaks an existing topology gets its own Breaking section in
[`CHANGELOG.md`](../CHANGELOG.md), with the migration named. That is the whole mechanism. A
new default counts as such a change when it refuses something an existing topology relied on:
`trusted_proxies` arrived that way in 0.45.0, and a proxy on another host needs its address
listed. The mailbox overflow is the other kind of breaking change, a behaviour an existing topology
may have leaned on: since 0.46.0 a full mailbox no longer holds back the producers of a cell, it
overflows per cell and only dead-letters above a cap, and a topology that used that back-pressure to
slow a producer sets the `colony.json` `mailbox_overflow_*` caps lower. The new dead-letter code it
brings, `mailbox_full`, is additive like any other. meclaw
has no deprecation period and no compatibility flag, so the release note is where you find out.

Nothing under `crates/` carries a SemVer guarantee. The Rust crates are internals and move
without notice, which is why the binary and the HTTP API are the interface this file talks about.

## Delivery across a crash

The colony's `message_log` is the source of every delivery, and a cell's mailbox is a cache of it.
A delivery the colony logged to a cell counts as consumed only once that cell's handler is done
with it (a normal return, a dead letter at the delivery gate, the `message_timeout` backstop, a
panic), never when the cell received it. When the process dies with deliveries that are logged and
not consumed, the next start hands each of them to its cell again, in log order, with the budget
(`ttl`) it was logged with, before the colony routes anything new. Order holds per cell, with one
exception: a cell that also had an overflow on disk gets its replayed mailbox deliveries behind
those overflow rows. A delivery that was replayed after three lives that crashed and finished on
none of them is the dead letter `replay_exhausted` on the next start; an ordered stop (the
shutdown teardown) does not count as an attempt, so restarts with a deep backlog never exhaust a
healthy delivery. The log keeps every row; replay deletes nothing.

A replayed delivery carries its original message id, and so does everything a replayed handler
emits: a follow-up's id is derived from the message it answers and its place among that handler's
emissions, not drawn at random. A follow-up whose id the log already holds is not delivered again,
so a chain of cells delivers each MESSAGE once to the next cell even across a crash. What is left
are effects: any cell whose handler finished but whose consume mark had not reached the disk sees
that delivery a second time — at any step of a chain, not only the last. The window lasts until
the writer commits the mark; the mark costs no commit of its own and queues behind everything the
writer already holds, so under load the window is the writer's channel backlog, not one batch.
A cell with an effect deduplicates by the message id: a stateful cell records the ids it consumed
in the same transaction as its effect, and an outgoing call carries the id as its idempotency key
where the far side takes one. The built-in cells:

| Cell | Deduplicated by message id | Window that remains |
|---|---|---|
| `store` | yes — writes book the id and their answer (`rows_affected`, payload) in `meclaw_consumed`, same savepoint as the write; a repeat writes nothing and answers what the first write answered, so a caller that branches on `rows_affected` (a compare-and-set claim) takes the same branch. Reads are not booked, the read legs of a bundle that writes included | none for writes; every read runs again, see below |
| `proxy` Telegram / Slack (outgoing) | yes — the id is booked in `consumed` before the platform call, the platform's answer after it; a repeat sends nothing and answers as the original did: silent after a confirmed send, the original's `send_failed` after a failed one, `send_failed` with the detail `unconfirmed: …` when the answer was never booked | a crash between booking and the call loses that message instead of sending it twice, and its repeat reports it as `unconfirmed` |
| `proxy` `meclaw` (peer) | yes — `peer_outbox` / `peer_inbox` and the frame id | see below |
| `llm` | no — the call runs again (cost); its follow-up emissions carry derived ids and are dropped when logged | one paid call per replayed delivery |
| `bash`, `process`, `file`, `edit`, `code`, `web` | no | the effect may happen twice within the window |
| `subcolony` | no | the child colony may receive the input twice |

A record booked before 0.61.7 holds the id without the answer. A repeat of such an id still writes or
sends nothing, but the store answers it as before the upgrade, `rows_affected: 0` and
`{"duplicate": true}`, and the Telegram and Slack proxies report it as `unconfirmed`.

A read leg in a bundle that also writes is not booked either. When such a bundle is repeated after
a crash, its write legs answer from the book and its read legs run again, so a read leg can see a
later state than the first answer saw: any write that went through between the first delivery and
the crash, including one from another message. A caller that writes and reads back in one bundle
(a claim followed by a look at the claimed rows, a value parked and read again) must tolerate a
read that is newer than its writes. Where it cannot, it reads in a message of its own after the
write's answer arrived, or checks what it reads against the write's own answer.

An ingress that holds a durable key for what it hands on stamps it into the hop as `delivery_key`
(the `meclaw` peer mount: its inbox key), and a hand-on it repeats from its own book after a start
as `delivery_replay: true`; the colony derives the id from the key and drops a repeat it already
logged — on every hop of the chain, through a hive transit too. A key that is a UUID (a frame id)
is the seed of the derived ids as it is; any other key is stamped with its arrival time as
`delivery_key_ms`, which becomes the ids' v7 time prefix. The `meclaw` peer mount books a handed-on
inbox row `done` only once it is older than 30 s and the next arrival comes in, because the mount
cannot see the colony's commit of the hand-on. Two consequences: if the colony's writer is backed
up for more than 30 s and the process then dies, a row booked `done` may have a hand-on that was
never logged — that delivery is lost; and a row without later traffic stays `pending`, so every
start raises it once more and the colony drops the repeat by its `delivery_key` (one primary-key
read), with no further effect. The durability covers a process crash, not a power loss (`synchronous = NORMAL`).
