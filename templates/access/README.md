# `access@2.1.0`

The capability broker: an agent may **ask in natural language**, what travels on the wire
is a **handle**, and no secret ever travels with a request. Built out of existing cell
types -- no new cell type, no Rust, and no model: every verdict in here is a comparison,
so a request costs two store hops and nothing else.

Six cells:

| path | type | role |
|---|---|---|
| `store` | `store` | policy, grants, grant_events, cred_refs, usage, audit |
| `policy` | `code` | the ONLY place a verdict is made -- deterministic, never a model |
| `invoke` | `code` | grant check, address resolution, and the one edge into the connector |
| `sweep` | `code` | TTL bookkeeping: writes the `expired` events |
| `clock` | `timer` | the sweep tick (6-field Quartz cron, **UTC**) |
| `vault` | `vault` | the credential VALUES, sealed at rest. Interior and unaddressable from outside; `params.broker` names `./invoke` as the only cell it answers, and two edges reach it (GH #421) -- what comes back is a sealed box, never a plaintext |

## The four messages

All four in the `tool_call` form, so an agent's dispatcher speaks them without a bridge
cell.

**1. `access.request`** -- the agent asks:

```json
{"capability": "chat.send",
 "subject":    "member:example",
 "resource":   {"channel": "example-chat", "chat_id": "…"},
 "purpose":    "answer the incoming message",
 "ttl_ms":     900000}
```

**2. `access.grant`** -- the answer, a `tool_result` on the same call id:

```json
{"grant_id": "grant:…", "status": "granted", "capability": "chat.send",
 "expires_at": "2026-08-15T09:15:00.000000Z",
 "constraints": {"max_invocations": 20},
 "scope_summary": "chat.send on example-chat (send_message)"}
```

What is **not** in it: the chat id, the `cred_ref`, a token, an endpoint. The model is told
*you may send on this channel* plus a handle -- and nothing more. On `status: "denied"` the
answer carries a `reason_code` so the agent can explain the refusal to a human instead of
guessing. The complete set the `policy` cell emits, in the order it can reach them:
`requester_unknown` (the edge promoted no `context.requester`), `args_not_json` (the tool
arguments are not a JSON object), `capability_missing` (the arguments name no capability),
`capability_unknown` (no rule mentions the capability at all -- and it is also what a rule
that mentions a *different* capability reports), `requester_mismatch`, `subject_mismatch`,
`scope_mismatch`, `scope_incomplete` (the first mismatch of the first rule examined wins),
`denied_by_rule` (a rule matched and its verdict is deny) and `approval_required`.

`no_rule` is the initial value of that reason variable and **cannot be observed**: an
empty rule set exits one branch earlier with `capability_unknown`, and any non-empty set
overwrites `no_rule` with the first rule's own mismatch reason. It is listed here because
it appears in the script, not because a caller will ever receive it.

**3. `access.invoke`** -- the execution:

```json
{"grant_id": "grant:…", "operation": "send_message",
 "payload": {"text": "on my way"}}
```

`invoke` checks, deterministically and in this order: the grant exists, its `requester` is
the caller, `now < expires_at`, `operation ∈ scope.actions`, the **newest** `grant_events`
row is a live one (`granted` / `approved` / `invoked` rather than `revoked` / `expired`),
and the constraints hold against `usage` (`max_invocations`, `rate_per_min`). Only then does
one message leave for the connector.

## 4. `access.invoke` with `operation: vault.deliver`

A credential VALUE can also be the thing that is spent. The request is an ordinary
`access.invoke` -- same lane, same grant, same four checks -- and it names an operation the
grant's `scope.actions` has to contain like any other:

```json
{"grant_id": "grant:…", "operation": "vault.deliver",
 "payload": {"recipient_key": "<64 hex chars, the requester's ephemeral X25519 public key>"}}
```

The answer is an ordinary `ack`:

```json
{"outcome": "ok", "grant_id": "grant:…", "operation": "vault.deliver"}
```

with one additional body slot beside it, which is where the credential actually rides:

```json
{"sealed": {"epk": "…", "nonce": "…", "ciphertext": "…"}}
```

**Which** credential is delivered stands in `grants.cred_ref` and **never** in the payload.
That is R-AC-2 applied to the vault: an address comes from the grant, and a secret's name is
an address. A payload that could name a secret would let one legitimate grant drain the whole
vault; the payload carries `recipient_key` and nothing else.

The crypto is short. The requester mints a fresh X25519 pair **per request** and sends the
public half as 64 hex characters. The vault mints its own ephemeral half **per answer**,
performs the X25519 agreement, derives the box key from it with HMAC-SHA256, and seals the
value with XChaCha20-Poly1305. There is no vault long-term key, so there is nothing an
attacker could later obtain that would open a box captured today.

A delivery is a **spend**. It writes the same `usage`, `grant_events` and `audit` rows as any
other invocation and it counts against `max_invocations` and `rate_per_min` exactly like one.
A refusal from the vault is booked and answered like every other refusal, with the vault's own
code passed through (`reason_code: "vault_locked"`, for instance).

Inside the hive, `./invoke` is the only cell the vault will answer a `deliver` for: the
operation is broker-only, and `params.broker` is what names the broker.

## The two invariants

These are not guidelines. They are the reason this hive is an authority rather than a
lookup table, and both are pinned in `crates/meclaw-cells/tests/access_template.rs`.

### R-AC-1 -- the requester comes from the EDGE, never from the body

A cell knows no sender. The only trustworthy origin in this substrate is a `set_context`
value on an **edge**, because edges are written by the colony (mutation authority) while a
body is written by whatever produced the message -- up to and including a model.

So `policy` and `invoke` both read `context.requester` and **never** an `arguments.requester`.
The field is not distrusted, it is not read. An absent `context.requester` is a **denial**
(`requester_unknown`), not a default: fail closed, because a broker that guesses its caller
has no callers it can name in an audit line.

> Both in-ports **MUST** carry `modifier.set_context.requester`. Wiring one without it is
> the single mistake that empties this template of meaning.

### R-AC-2 -- the address comes from the GRANT, the content from the payload

`invoke` builds the destination out of `grants.scope` and out of nothing else. Every
payload key that names an address coordinate is **removed** before the message reaches the
connector -- not merged, not preferred, removed -- and the removal is recorded in the audit
detail under `payload_address_ignored`. Without this, a model holding one legitimate grant
could write to any chat it can name.

The scope itself is frozen at grant time: a rule's `scope_match` decides which coordinates
exist at all, a literal comes from the rule and a `*` from the request. A coordinate the
rule never mentioned is not part of the grant, and therefore not part of any address.

## Cells and lanes

```
access/                       hive  -- scope marker, sealed; fourteen edges in params.graph
  store/                      store -- six tables
    seed/{policy,cred_refs}.jsonl
  policy/                     code  -- request -> allow | deny | require_approval
  invoke/                     code  -- grant check -> address from grant -> connector
  sweep/                      code  -- TTL: writes expired events
  clock/                      timer -- the tick
  vault/                      vault -- the credential values; sealed delivery to ./invoke
```

A `code` cell has no `cell.db`, so a lane that needs several reads keeps its state on the
wire: each cell emits its phase and its carry on the **hop**, the internal edge promotes
both to **context** (`ac_phase` / `ac_carry`), and the store's answer brings them back. The
store round trip *is* the cell's memory. Of the fourteen edges in `params.graph`, nine are
interior: six store round trips (three cells, each with a leg out and a leg back), the clock
tick into `sweep`, and the vault round trip (GH #421) -- `./invoke -> ./vault` on the `avault`
lane and `./vault -> ./invoke` back. The other five are the hive's own door -- two in, three
out.

The vault pair is the store mechanism one context key further along: the outbound edge
promotes `access_origin: 'invoke'`, `access_lane: 'vault'`, `ac_phase` and `ac_carry` into the
context, and the return edge conditions on
`context.access_origin == 'invoke' && context.access_lane == 'vault'`. This reverses GH #307,
and this time there is a real emitter behind it -- `2.0.0` wired a pair that waited for a
route no cell ever produced, `2.1.0` wires a lane a cell actually drives.

### Lanes

`params.ports` is empty: **the hive path is the only address**, and what a caller
asks for rides on `hop.route` (overview § Die Hive-Grenze). Which cell serves a
lane is stated once, on this hive's own door edge, and nowhere else.

| lane | direction | carries |
|---|---|---|
| `in_request` | in | `access.request` as a `tool_call` turn; the edge **MUST** promote the caller to `context.requester` |
| `in_invoke` | in | `access.invoke` as a `tool_call` turn; the edge **MUST** promote the caller to `context.requester` |
| `grant` | out | the verdict, with `hop.verdict` and `hop.grant_id` beside it |
| `ack` | out | `ok` or `denied` plus a `reason_code`; a sealed credential leaves here too, recognisable by `hop.operation == "vault.deliver"` and the extra `sealed` body slot |
| `connect` | out | `hop.address` is the grant's scope as canonical JSON, `hop.channel` and `hop.operation` beside it, the body carries the payload minus every address key |
| `error` | out | a lane failed -- the parent **MUST** wire it |

`avault` is **not** in this table, and that is the point: it is a hive-INTERNAL lane between
`./invoke` and `./vault`, invisible from outside the scope. Sealed delivery added **no** hive
lane -- the box rides the existing `ack`, because an `ack` *is* the result of a spend and a
delivery is a spend. So the hive contract is lane-identical to `2.0.5`, and a caller wired
against that version keeps working unchanged.

So a caller's wiring names the hive twice and an interior never:

```json
{"from": "./agent", "to": "./access",
 "modifier": {"set_hop": {"route": "'in_request'"},
              "set_context": {"requester": "'agent:example'"}}}
{"from": "./access", "to": "./agent",
 "condition": "has(hop.route) && hop.route == 'grant'"}
```

The connector is **not** part of this template. It is whatever cell holds the credential --
a `proxy` with a late-bound token, for instance -- and it stays dumb on purpose: it knows no
grant, only an address and a value.

### Migrating from `access@1.x`

`1.x` declared `params.ports: ["policy", "invoke", "store"]`. All three are
retired, and the third one was the bypass this README's own *honest limit*
calls out: a declared port straight into the store is an invitation to write a
policy row without asking. Drop the last segment and name the lane --
`./access/policy` becomes `./access` with `set_hop.route: "'in_request'"`,
`./access/invoke` becomes `./access` with `"'in_invoke'"`, and an outbound edge
starts at `./access` and conditions on `grant`, `ack`, `connect` or `error`.

## When the broker is mandatory, and when it is not

**A reply inside a running conversation stays an ordinary edge** (ruling L5). The reply lane
is already a capability statement: it exists because the topology allows it, and it can only
reach the chat the turn came from. Putting a broker in that path would buy nothing and add a
second failure mode to the hot path.

The broker is mandatory where the agent wants a **new address** or a **new capability** --
a reminder, a push, a chat nobody wrote from, a second platform. There the question is real,
and there a `denied` is a useful answer.

So this template is **additive**. Nothing that runs today has to be re-cut to adopt it.

## Policy is rows, and it ships switched off

A rule is a row. Politics is changed with an `insert` or an `update`, live, without a
restart and without a deploy:

| column | meaning |
|---|---|
| `requester` | `agent:example`, `member:example` or `*` |
| `capability` | `chat.send`, `web.fetch`, … or `*` |
| `subject` | on whose behalf, or `*` |
| `scope_match` | `{"channel": "…", "chat_id": "*", "actions": ["send_message"]}` |
| `verdict` | `allow` / `deny` / `require_approval` |
| `max_ttl_ms` | the ceiling; the request may ask for less, never for more |
| `constraints` | `{"max_invocations": 20, "rate_per_min": 6}` |
| `cred_ref` | which credential the grant is bound to -- a REFERENCE, never a value |
| `enabled`, `priority` | off/on, and the order rules are examined in |

Every seeded row ships `enabled: 0`. A fresh instance therefore **grants nothing** and
answers `capability_unknown` until an operator turns on exactly what they meant to. The
seed exists to be read, not to authorise.

Rules are examined in `priority` order and the **first match wins**. An explicit `deny` is
worth writing precisely because it reads differently from silence: `denied_by_rule` tells a
caller it found a closed door, `capability_unknown` tells it there is no door there at all.

`require_approval` reports `status: "pending"` and mints **no** grant. The approval lane
itself is not in v1 -- decision B-4 is unruled, and a pending grant nothing can approve
would be a grant that says yes by accident.

## Revocation, expiry, and why neither is a column

The effective state of a grant is **its newest event**:

```json
{"operation": "select", "table": "grant_events", "columns": ["event", "at"],
 "where": {"grant_id": "grant:…"},
 "order_by": [{"col": "at", "dir": "desc"}], "limit": 1}
```

A revocation is an **append** -- one `revoked` row -- and it takes effect on the very next
call, `expires_at` notwithstanding. Nothing in this template ever emits `update` or `delete`
against `grants` or `grant_events`, so the history stays complete and *who was allowed to do
what on the third* is one query. The price is one store hop per check, which at one grant
per conversation is nothing.

Timestamps are ISO-8601 UTC with **microseconds**, and the fixed width is load-bearing:
`at` and `expires_at` are compared as text, so lexicographic order has to be chronological
order. Second precision would let two events of the same turn tie, and a tie in a revocation
history is a security defect rather than a cosmetic one.

**The TTL bites on the call, not on the tick.** `invoke` compares `expires_at` itself, every
time. `sweep` only writes the `expired` row afterwards, and it is idempotent (a grant whose
newest event is already `expired` is skipped). A colony whose timer never fires is behind on
its bookkeeping -- it is not open on its door.

## Secrets: `./vault` holds the values, `./store` holds the catalogue

The hive ships a `vault` cell, and it is where a credential VALUE rests: XChaCha20-Poly1305
per secret, argon2id from a passphrase whose source is named by `params.key_source`
(`auto` | `prompt` | `systemd-cred` | `plainfile`) and never by material in a config. The
route surface of the type has no `get` -- `put`, `rotate`, `use`, `deliver`, `revoke`,
`status`, `unlock`, `lock`, and nothing else -- so a fully compromised model on the far side
of an edge can ask the vault to USE a secret and cannot ask to see one in the clear. A woken
vault is always locked; the key lives in the task and dies with it.

**A value leaves the vault sealed, or it does not leave.** `deliver` (GH #421) is the one
route that hands a credential out, and what it hands out is a box: the requester's ephemeral
X25519 public key goes in, `{"epk", "nonce", "ciphertext"}` comes back, and the plaintext
exists nowhere on the wire and nowhere in the `message_log`. The vault's own half of the
agreement is minted per answer and discarded with it, so there is no long-term key whose
compromise would open yesterday's box.

`params.inject_map` -- the older path, a plaintext push to a connector cell at unlock time --
is therefore **deprecated**. It writes a clear credential into the `message_log`, which is
precisely what sealed delivery exists to prevent. It is not removed, because removing it
would be breaking; it goes with the first release that bundles breaking changes. It remains
empty on a fresh instance, because a vault that shipped with delivery addresses would deliver
to somebody else's.

`./vault` is **not** a port of this hive. `params.ports` is empty, so the generic boundary
refuses any edge from outside the scope onto it; the only cell it answers on the broker
channel is `./invoke`, which is what `params.broker` says. On top of that the vault verifies
its own inbound edges against `broker` + `sealed_neighbors` before it accepts key material,
and stays locked if the neighbourhood is not the one it expects.

Since GH #421 that attestation is **stricter**: a vault whose broker edge is missing no longer
attests at all and stays locked, with reason `broker_unwired`. The reasoning is the same one
that lets the box go unsigned -- if the topology is what vouches for the sender, then the
absence of that topology cannot be allowed to vouch for anything.

Since GH #314 `vault/config.json` also declares `contract.transfer: "none"`. The route surface
without a `get` was only half the promise while the `transfer` body slot sat above it: the
**substrate** answers that slot before `handle()`, so the vault's two-caller ACL never saw one,
and an `export` needed no passphrase to be worth sending -- `vault_secrets` carries `name`,
`version`, `status` and `created_at` in plaintext, and `vault_audit` is the whole call history.
The declaration makes the cell exempt from the slot in both directions (`transfer_exempt`);
`contract.write_surface` would not have covered it, because an export is a read.

The hive did draw **no** edge to it between `access@2.0.1` and `2.0.5` (GH #307), and the
reason is worth keeping: `2.0.0` shipped a pair -- `./invoke -> ./vault` on
`hop.route == 'vault'` and the reply back -- and neither could ever fire, because `invoke`'s
script emitted four literal routes (`astore`, `ack`, `error`, `connect`) and none of them was
computed. Nothing in this hive ever produced the route the edge waited for. Dead wiring reads
as a channel that carries something, so it went.

`2.1.0` draws the pair again, on the `avault` lane and with an emitter behind it (GH #421).
`params.broker` therefore names a cell that **is** answered rather than one that would be, and
the vault no longer attests with an empty neighbourhood.

There is still **no** `secrets.db` in `./store`, and there is not meant to be one: an
encrypted table there would be a second vault with a worse key story. A variable reference
binds late (F1, 2026-08-13) for the connector that is configured that way: the token stands
literally in the connector's `config.json`, its value exists only in that cell's memory, and
it reaches no other config, no message, no `message_log` and no error text.

So `cred_refs` records **where** a secret lives and under which environment variable
**name**, never what it is:

| `ref` | `connector` | `env_var` | `owner` | `status` |
|---|---|---|---|---|
| `cred:example-chat:primary` | `./connector/proxy` | `EXAMPLE_CHAT_TOKEN` | `member:example` | `active` |

The names in this table are written as bare identifiers on purpose. A literal
`${EXAMPLE_CHAT_TOKEN}` in a seed row would be **resolved at bootstrap** and would write the
actual secret into `cell.db` -- the precise opposite of what a catalogue is for. `rotated_at`
is bookkeeping: the rotation itself happens in `.env` plus a restart of the connector cell.

## The honest limit

> The broker is exactly as strong as the sentence: **there is exactly one edge into the
> connector, and it comes out of this hive on the `connect` lane.**

There is no permission layer in this substrate. Whoever can route, may. `capabilities` in a
contract are discovery hints and not runtime checks. So if any cell is given an edge to the
connector, it sends without a grant, and nothing here will notice. What the seal DOES buy
is the other half of that sentence: since `access@2` a parent scope can no longer draw an
edge straight into `./store` and write a policy row itself -- a mutation that tries is
refused with `hive_port_boundary`. `access@1` declared `store` as a port and therefore
invited exactly that.

What the seal does **not** buy is the rest of the way into that database, and the residual is
named here rather than papered over. A **bootstrap** `params.graph` of a parent still can draw
the edge: the birth topology is the colony author's sovereign design, and the seal guards
against runtime mutation. On top of that the write surface has had two halves since GH #260 --
`params.write_surface` bounds the ops the store's own `handle()` runs, `contract.write_surface`
bounds the `import` of the `transfer` body slot, which the **substrate** answers before
`handle()` is ever reached. `store/config.json` declares the **contract** half: without it an
`import` writes `policy` rows in bulk, from any sender, straight past every comparison this hive
is built on.

Since `access@2.0.4` `store/config.json` also declares `contract.transfer: "none"`
([#336](https://github.com/mmeyerlein/meclaw/issues/336)) -- the mechanic of GH #314, the same
one `./vault` carries. It closes the half `write_surface` deliberately leaves open: an **export
is a read**, so the #260 declaration never bounded it. What the read half handed out was the
broker's whole state in one answer -- `grants`, where every row is a live **bearer** handle,
`cred_refs` with the variable name behind every connector, and the complete `audit` history of
who asked for what and what was refused. The exemption is answered by the substrate *before* the
arguments are read, so the refusal (`transfer_exempt`) is the same sentence for every question
and names no table: one that said `unknown_table` for one name and something else for another
would itself be an inventory.

**The migration story for this hive is therefore re-granting at the target, not importing.** A
`grant_id` is a bearer instrument; a migrated grant is a *copied* bearer instrument, live at both
ends and revocable only where its `grant_events` rows are -- there is no honest way to move one.
`policy` and `cred_refs` need no export either: they are catalogue and ship as a **seed**
(`store/seed/*.jsonl`), so a fresh instance starts from the seed -- inert, every rule `enabled: 0`
-- and an operator enables exactly what they mean at the new address. `usage` and `audit` are the
record of a broker that ran at the old one; they stay there.

`clock/config.json` declares it too, since `access@2.0.3`
([#332](https://github.com/mmeyerlein/meclaw/issues/332)), and not out of symmetry: a timer's
`cell.db` **is** its schedule list. An imported `schedules` row is a firing with an `emit_to`
of the writer's choosing -- the clock calling someone else's number, with a body nobody in
this hive decided about, on a cadence nobody set. The `params` half has no meaning for a
`timer`; the substrate one is the only write surface it has, and it is closed.

It deliberately does **not** declare the `params` half, and that is a property of this template
rather than an oversight: **no cell inside the hive ever writes `policy` or `cred_refs`.**
`policy`, `invoke` and `sweep` only `select` from those two tables; what they write is `grants`,
`grant_events`, `usage` and `audit`. Turning a rule on is the operator's gesture and it comes
from outside the scope by construction, so a cell-level seal would not tighten this boundary --
it would leave a freshly instantiated broker inert forever, with no path to ever enable
anything. The half that can be closed without breaking that is closed; the other one stays a
sentence in this README, which is the same soft sovereignty `affinity` names for its own
residual.

Five further limits, named rather than papered over:

- **No encryption at rest** for `cell.db`, and no key rotation in the substrate.
- **The `usage` page is bounded** (`ACCESS_USAGE_ROWS`, default 500). A `max_invocations`
  above that bound cannot be enforced.
- **No grant context in an agent's system prompt.** That an agent knows it has to ask is
  prompt and tool design, not something this template can arrange.
- **A sealed box does not prove WHO sealed it.** It proves that whoever did held the
  requester's ephemeral public key, and nothing further. What carries authenticity here is the
  topology -- the vault answers `params.broker` and no one else -- plus the policy the broker
  enforced before the delivery was booked. That is a deliberate trade, not an omission: a
  signature is addable later as a fourth field beside `epk`, `nonce` and `ciphertext`, without
  breaking the wire form.
- **A vault inside a sealed hive cannot be unlocked today**
  ([#427](https://github.com/mmeyerlein/meclaw/issues/427), open). `unlock` is user-channel-only
  in the vault's ACL, and a user-channel message is by definition a source message -- it carries
  no `reply_to`. A source message reaches no hive-interior cell; the only thing that reaches one
  is an edge, and an edge always carries `reply_to` and is therefore never the user channel. The
  two rules are each sound and together they close the door. This is an open finding, not a
  design goal.

What the template *does* guarantee is that everything which passes through it is decided by
a comparison, recorded in a row, and addressed from a grant.

## Settings

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

| variable | default | meaning |
|---|---|---|
| `ACCESS_SWEEP_CRON` | `0 */5 * * * *` | 6-field Quartz cron of the TTL sweep, **UTC** |
| `ACCESS_POLICY_ROWS` | `200` | page bound of one rule read |
| `ACCESS_MAX_TTL_MS` | `86400000` | the ceiling no rule can raise |
| `ACCESS_USAGE_ROWS` | `500` | page bound of the quota read |
| `ACCESS_SWEEP_ROWS` | `200` | grants examined per tick |
| `ACCESS_SWEEP_EVENT_ROWS` | `2000` | event page per tick |
