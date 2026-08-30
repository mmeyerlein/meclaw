# `daily-digest@2.0.3`

Scheduled fetch-and-forward: timer → web_fetch → format (code) → Telegram proxy.

```
cron --(schedule fires, tool_call {url})--> fetcher --(2xx)--> format --> notifier --> [telegram]
                                              ^                  |
[caller] --(in_digest)------------------------+                  +--(digest)--> [caller]
                                                       (whichever of the two the run came from)
```

## Flow

1. `cron` (timer, 6-field Quartz `0 0 8 * * *`) fires its schedule: the body is a static
   `tool_call` carrying `{url: $DIGEST_URL}`, the headers carry `chat_id`
   (`$TELEGRAM_DIGEST_CHAT_ID`, string at this point).
2. Edge `cron → fetcher` promotes `chat_id` into `context` — hop dies at the next
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

## Required environment (`.env`)

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

| Variable | Purpose |
|---|---|
| `TELEGRAM_BOT_TOKEN` | Bot token |
| `TELEGRAM_DIGEST_CHAT_ID` | Target chat (numeric id as string) |
| `DIGEST_URL` | Optional — defaults to `https://example.com` |

## Activation (island template)

This is a self-contained island (internal edges only). Since A7 (2026-06-12) it
instantiates **inactive** — the `cron` timer does not spawn until the digest hive is
edge-connected to an active scope. Activate it the standard way: in the `add_nodes`
instantiation mutation, add ONE crossing edge from an active parent cell to the **hive
path** — `./daily-digest`, the same address every lane below uses (it may be
connectivity-only). The subtree then derives active and the timer fires on its schedule —
no re-root.

The old wording here named `./digest/cron`, and both halves of that were wrong. `digest`
is not the instance name — an instance is named exactly like its template — and an
interior cell is not an address: `params.ports` is empty, so a later mutation naming
`./daily-digest/cron` is refused with `hive_port_boundary`. (Inside the *instantiating*
mutation itself the hive is not yet in the sealed set, so the old form would commit —
which is precisely what let the wording survive.)

See the gate runbook § Island activation.

## Status

All gates pass against core tag `post-migration-substrate-fixes` (timer/proxy factories
wired, proxy inbound reads the header compartments). Live run needs real tokens in the
repo-root `.env`.

## Lanes

`params.ports` is empty (GH #228): the address is the hive path itself, and what a caller
wants rides on `hop.route`.

| lane | direction | what travels |
|---|---|---|
| `in_digest` | in | a digest run demanded now, outside the timer -- the same body the schedule emits, one `tool_call` turn whose text is the JSON arguments carrying the URL. `chat_id` must already be in `context`, as a string |
| `digest` | out | the formatted digest of a demanded run, handed back to whoever demanded it |

**The clock keeps its own path.** A scheduled run goes `cron -> fetcher -> format ->
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
