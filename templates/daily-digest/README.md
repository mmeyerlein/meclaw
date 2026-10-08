# `daily-digest@2.2.2`

Scheduled fetch-and-forward: timer → web_fetch → format (code) → Telegram proxy.

```
clock --(schedule fires, tool_call {url})--> fetcher --(2xx)--> format --> notifier --> [telegram]
                                               ^                  |
[caller] --(in_digest)-------------------------+                  +--(digest)--> [caller]
                                                        (whichever of the two the run came from)
```

## Flow

1. `clock` (timer, 6-field Quartz `0 0 8 * * *`) fires its schedule: the body is a static
   `tool_call` carrying `{url: <params.digest_url>}`, the headers carry `chat_id`
   (`<params.chat_id>`, string at this point).
2. Edge `clock → fetcher` promotes `chat_id` into `context` — hop dies at the next
   emission — and stamps `context.digest_origin = 'schedule'`, which is what keeps a
   scheduled run on the notifier and off the `digest` lane.
3. `fetcher` (web_fetch, GET only) pulls the URL; non-2xx has no edge and dead-letters.
4. `format` (code) turns the `tool_result` into an assistant turn ("Daily digest:" +
   3500-char excerpt) and recasts `chat_id` to a number in its hop; the out-edge
   re-promotes it to `context`.
5. `notifier` (proxy) sends the assistant turn to `context.chat_id`. It is a pure
   delivery sink — its `emit_to` points at the silent `void` terminal, so stray user
   messages to a push-only digest bot are swallowed rather than routed anywhere.
6. A run that was demanded on the `in_digest` lane carries
   `context.digest_origin == 'parent'` instead, and the complementary edge `format → .`
   hands the formatted digest back out of the hive on `hop.route == 'digest'` rather
   than into this bot's chat. See [Lanes](#lanes).

## What it takes, and from where

**Since 2.1.0 both behaviour knobs are params of `./clock`, not environment variables**
([#138](https://github.com/mmeyerlein/meclaw/issues/138), ruling R-0904-6). They live inside
`params.schedules`, which is one key holding the whole list, so an `override_params` entry
names `schedules` and replaces it -- last-write-wins, and `contract.settings` declares what
each field means.

| param of `./clock` | default | purpose |
|---|---|---|
| `digest_url` | `https://example.com` | URL fetched on every firing, inside the schedule's `tool_call` args |
| `chat_id` | *(empty)* | target chat of a SCHEDULED run, on the schedule's `emit_headers` |

**The chat id ships EMPTY, and that is a change worth reading twice.** Under the environment
form `TELEGRAM_DIGEST_CHAT_ID` had no default, so a colony that did not set it refused to
boot and named the variable. A params default cannot refuse. What replaces the refusal is
this template's own shape: the hive instantiates **inactive**, so nothing fires until a
crossing edge is drawn, and the mutation that draws it is the mutation that names the chat.
An empty chat id delivers nowhere rather than somewhere wrong.

## Credentials

No key lives in `.env` or in this tree (#801). The grants are seeded with the template (`access/store/seed/`); deposit each credential once, on stdin:

| credential | cells |
|---|---|
| `cred:telegram-bot` | `notifier` |

```sh
meclaw --root <root> --vault <instance>/access/vault --vault-add <credential>
```

The vault must be able to unlock itself: `access/vault` `key_source` `systemd-cred` for a unit, `plainfile` for a local run.

```json
{"add_nodes": [{"name": "daily-digest", "template": "daily-digest",
  "override_params": {"clock": {"schedules": [{
    "schedule_id": "0190a3f2-0000-7000-8000-0000000004d1",
    "schedule_name": "daily-digest", "cron": "0 0 8 * * *",
    "emit_to": "../fetcher",
    "emit_body": {"messages": [{"origin": "assistant", "type": "tool_call",
                                "id": "digest-fetch",
                                "text": "{\"url\": \"https://example.org/feed\"}"}]},
    "emit_headers": {"chat_id": "-1001234567890"}}]}}}]}
```

## Activation (island template)

This is a self-contained island (internal edges only). Since A7 (2026-06-12) it
instantiates **inactive** — the `clock` timer does not spawn until the digest hive is
edge-connected to an active scope. Activate it the standard way: in the `add_nodes`
instantiation mutation, add ONE crossing edge from an active parent cell to the **hive
path** — `./daily-digest`, the same address every lane below uses (it may be
connectivity-only). The subtree then derives active and the timer fires on its schedule —
no re-root.

The old wording here named `./digest/cron`, and both halves of that were wrong. `digest`
is not the instance name — an instance is named exactly like its template — and an
interior cell is not an address: `params.ports` is empty, so a later mutation naming
`./daily-digest/clock` is refused with `hive_port_boundary`. (Inside the *instantiating*
mutation itself the hive is not yet in the sealed set, so the old form would commit —
which is precisely what let the wording survive.)

See the gate runbook § Island activation.

## Since 2.1.0: the timer is called `clock`

The cell that used to be `cron` is `clock`. The schedule, the `tool_call` body,
the `chat_id` header and the out-edge to the fetcher are what they were; only
the name moved. `cron` named the mechanism rather than the job, and the library
shipped six spellings of the same cell
([#551](https://github.com/mmeyerlein/meclaw/issues/551) § 2): a `timer` is a
`clock` unless its NAME says which tick it is. The payload rides on the
schedule, not on the cell name. The hive is sealed (`params.ports` is empty), so
no caller could ever name the cell — which is why the second digit moved rather
than the first.

**A standing instance keeps the name it was grown with.** Instantiation copied
the subtree, so a digest hive grown from an earlier version still holds a `cron`
directory and still fires; a path IS a cell's identity and only `move_nodes`
changes one. Renaming it is an operator's act and is never required.

## Since 2.1.1: `void` runs warm

`./void` is the terminal of the notifier's inbound lane: every chat message the bot is
sent ends there. It declares `runner_mode: "warm"` since `2.1.1`
([#852](https://github.com/mmeyerlein/meclaw/issues/852)) -- one interpreter that stays,
a fresh namespace per message, the same `[]` out. The script keeps no state and reads
stdin as text only (`json.load(sys.stdin)`), so the move changes cost and nothing else.

## Since 2.2.0: the last digest is kept, with its round ([#976](https://github.com/mmeyerlein/meclaw/issues/976))

Every formatted digest also goes to `./shelf`, which keeps it as one row of `./store`
(`digests {id, at, audience_set, title, items}`) beside the delivery. The row's round is the
round of the run that made it: the caller's `context.audience_set` on `in_digest` (a demanded
run without one is kept with an empty round, which covers no screen -- never shown, never
widened to `*`). A scheduled run belongs to the member that holds this app: it is kept with the
holder mark and read back under the member's round that the builder's edge of the read stamps,
so it shows on a screen of the member's round and on no other -- a round neither the app nor a
caller can set.

A read on `in_read` (hop `op` = `last`, `op_id` mirrored) answers once on `answer` with
`{ok, op, digests: [{id, at, when, title, items, lead, audience_set}]}`, newest first, holding
only the digests whose round covers the round of the read (`context.audience_set`, written by
the builder's edge of the residents' road). A read without a round reads no row
(`{ok: false, error: {code: no_round}}`). This is how a presenter shows the last digest as its
built-in topic `digest`, only to the round it stems from. A new lane a caller can use is the
second digit.

## Status

All gates pass against core tag `post-migration-substrate-fixes` (timer/proxy factories
wired, proxy inbound reads the header compartments). Live run needs the bot token
deposited in the vault.

## Lanes

`params.ports` is empty (GH #228): the address is the hive path itself, and what a caller
wants rides on `hop.route`.

| lane | direction | what travels |
|---|---|---|
| `in_digest` | in | a digest run demanded now, outside the timer -- the same body the schedule emits, one `tool_call` turn whose text is the JSON arguments carrying the URL. `chat_id` must already be in `context`, as a string |
| `digest` | out | the formatted digest of a demanded run, handed back to whoever demanded it |
| `in_read` | in | a read of the kept digests (hop `op` = `last`, `op_id`), with the round of the read in `context.audience_set` -- the residents' road of a presenter (GH #976) |
| `answer` | out | the one answer to an `in_read`: the kept digests the round of the read may see, each with its own round, `op` and `op_id` mirrored |

**The clock keeps its own path.** A scheduled run goes `clock -> fetcher -> format ->
notifier` and never touches these lanes; a demanded run enters at the fetcher and leaves
at the hive path instead of being pushed into this bot's chat. The two are told apart by
`context.digest_origin`, which the two ingress edges stamp (`'schedule'` / `'parent'`)
and the two exit edges read.

`chat_id` is the one thing the caller has to bring. The formatter recasts it from a
context string into the delivery hop and cannot invent one, which is why the lane
declares it in `accepts[].context`. Since [#291](https://github.com/mmeyerlein/meclaw/issues/291)
that declaration is checked: an edge that names this lane without `chat_id` promoted on it,
or reachable backwards from its own `from`, is refused `hive_contract` before anything is
wired.

```json
{"from": "<caller>", "to": "./daily-digest",
 "modifier": {"set_hop": {"route": "'in_digest'"},
              "set_context": {"chat_id": "hop.chat_id"}}},
{"from": "./daily-digest", "to": "<caller>",
 "condition": "has(hop.route) && hop.route == 'digest'"}
```

Which cell fetches and which one formats is this template's business and may change
without a caller noticing.
