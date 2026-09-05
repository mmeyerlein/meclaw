# `access@2.5.0`

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

One optional field: `"check_only": true` asks for a **verdict** rather than for a grant.
Nothing is minted, `ttl_ms` is ignored, and the answer's `status` is `allowed` instead of
`granted` (see *Asking whether, without asking for*, below).

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

### Two questions in a row, and the marker that used to answer both

A caller may ask more than once on **one message chain** -- `submit` asks up to three times
per manifest, sequentially. Until `2.4.1` the second question was always refused, with
`capability_unknown`, stamped with the capability and the `call_id` of the **first** one, and
the `policy` table was never read for it at all (GH #481).

The cause was the mechanism above, used one step too far. `context` is persistent for the
life of a chain, and the three keys the interior edge promotes had nothing that removed them
again: they survived the store round trip, the `grant` emission, the trip out of the hive and
the caller's next move. The second request therefore arrived already carrying
`access_origin: 'policy'` and a stale `ac_phase` / `ac_carry`, and the cell read it as its
own echo -- so it answered out of the previous round's carry instead of asking the table.

Two rules keep it fixed, and each of them is enough on its own:

- **A delivery on an inbound lane is a REQUEST**, whatever the context carries. `./policy`
  and `./invoke` recognise `hop.route == 'in_request'` / `'in_invoke'` first, and a store or
  vault answer never carries a `hop.route` at all. This is the same discipline the marker was
  introduced for -- recognise **positively**, and by something the colony wrote -- applied in
  both directions rather than one.
- **The markers never leave the hive.** All three exit edges (`./policy -> .`,
  `./invoke -> .`, `./sweep -> .`) clear `access_origin`, `access_lane`, `ac_phase` and
  `ac_carry` with `delete_context`. An interior state key that crosses a sealed boundary is
  leakage even when nobody reads it -- and here somebody did: this hive, one question later.

This grants nothing that was refused before. It only makes the second question get asked;
what the answer is, is still whatever the rows say. Pinned in
`crates/meclaw-cells/tests/gh481_the_broker_answers_the_question_it_was_asked.rs`, over the
hive's real edges -- driving the script directly cannot see an edge, which is why nothing was
red for as long as the path existed.

### Lanes

`params.ports` is empty: **the hive path is the only address**, and what a caller
asks for rides on `hop.route` (overview § Die Hive-Grenze). Which cell serves a
lane is stated once, on this hive's own door edge, and nowhere else.

| lane | direction | carries |
|---|---|---|
| `in_request` | in | `access.request` as a `tool_call` turn; the edge **MUST** promote the caller to `context.requester` |
| `in_invoke` | in | `access.invoke` as a `tool_call` turn; the edge **MUST** promote the caller to `context.requester` |
| `grant` | out | the verdict, with `hop.verdict` and `hop.grant_id` beside it. `hop.verdict` is one of `granted`, `allowed` (a `check_only` request — a yes with no grant behind it, so `hop.grant_id` is empty), `pending` or `denied` |
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

### A wired example: `examples/vault-pilot` (GH #452)

This template ships six cells and no deployment. `examples/vault-pilot/` is the first
one: one `llm` cell whose `params.credential_grant_id` names a grant, an
`unlock_env` set through the manifest, one `cred_ref`, one enabled rule, and one
long-term grant with its `granted` event. It grows in a single `meclaw --apply`.

Two facts about it are facts about **this** template, and they are the reason it
looks the way it does.

**A grant cannot be born in a manifest.** `credential_grant_id` is immutable, so the
`grants` row has to exist before the consumer boots — and the mutation diff
vocabulary is seven topology operations, none of which writes a store row. The only
manifest-reachable way rows enter a store is a `seed/<table>.jsonl`, and that lands
exactly once, on a fresh `cell.db`. The example therefore checks THIS hive's
`./store` in with its own seed and lets the instantiation merge around it: a subtree
cell that already lies on disk is left untouched, and the other five come from here.
So the ergonomic gesture that would write policy row, `cred_ref`, grant and event in
one go does not exist yet, and the honest substitute is a seed file plus a
derivation rule for the handle.

**`params.unlock_env` had to become a declared key.** It ships `null` — the default
truth stays *a woken vault is locked* — but a key that is not declared names no
param, and `override_params` refuses a name it cannot find. Before that, the one
setting every deployment has to make was the one setting no instantiation could
make; the value still comes from the environment and never from a config.

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
| `scope_match.scope_prefix` | a reserved key: a **path prefix** matched against `resource.scope` |
| `verdict` | `allow` / `deny` / `require_approval` |
| `max_ttl_ms` | the ceiling; the request may ask for less, never for more |
| `constraints` | `{"max_invocations": 20, "rate_per_min": 6}` |
| `cred_ref` | which credential the grant is bound to -- a REFERENCE, never a value |
| `enabled`, `priority` | off/on, and the order rules are examined in |

Seven rows ship: three examples and four the submitter asks over. **Five of the seven
ship `enabled: 0`** -- the three examples, `code.author.default` and (since `2.4.2`)
`colony.mutate.shell` -- and a fresh instance answers `capability_unknown` or a
`scope_mismatch` for every one of them until an operator turns on exactly what they meant
to.

**Two ship `enabled: 1`, and they are the two a colony cannot start without** (ruling
R-Policy-Default, 2026-08-28): `colony.mutate.default` and `affinity.subscribe.default`.
A freshly instantiated OS has to be able to build, and its brains have to be able to
register for their own identity; a default that refused both would make the first mutation
of every colony an operator step and a shipped agent silent until somebody remembered a
`UPDATE policy`. The two are narrow rather than open -- requester `/os/operator/submit`, action
`apply`, scoped to `/os/orgs` -- and what they do NOT grant is the sharp part:
`code.author` stays off, so a manifest that carries a script nobody reviewed is still
refused on a fresh tree. Narrow these two (a `subject`, a tighter `scope_prefix`) rather
than switching them off, and set `enabled: 0` if this colony wants no default at all.

The first is `colony.mutate.default` — `requester: "/os/operator/submit"`, `subject: "*"`,
`scope_match: {"scope_prefix": "/os/orgs", "actions": ["apply"]}`. It is the row that
lets a manifest reach the mutation door, and R-AC-1 is what shapes it: the requester is
the **submit hive**, promoted by the edge, and the identity on whose behalf it asks
travels as `subject`. The delegation stands visibly in a row instead of implicitly in a
script.

The second is `code.author.default`, and it answers a different question about the same
manifest (GH #446). `colony.mutate` is *who may submit*; `code.author` is *whether this
submission may bring executable behaviour with it*. The submitter derives the need from
the DIFF rather than from a prompt — an `override_params` (or a `swap_nodes` `with`)
carrying `script_inline` / `script_path`, or an `add_templates` at all — and asks a
second check-only question over the same scope root, with the same shape:
`requester: "/os/operator/submit"`, `subject: "*"`,
`scope_match: {"scope_prefix": "/os/orgs", "actions": ["apply"]}`.

The two are apart because they are apart in fact: an operator can grant one and refuse the
other, and a single verdict could only ever have said yes to both. Until this row exists
and is enabled, a manifest that authors code is refused with `code_author_denied` — the
broker answers `capability_unknown`, and a missing rule is a denial rather than a silence.
That was the promise; until `2.4.1` the second question never reached the table at all, so
an enabled row changed nothing (see *Two questions in a row*, above).
Disabled on arrival, like everything else here: a fresh colony applies manifests and
authors no code. What made this necessary is that the prohibition used to live in one line
of a drafting prompt, and nothing in the tree enforced it.

The third is `affinity.subscribe.default`, and it is where the boundary of this whole
table becomes visible (GH #458). The `in_pack` lane is the only door through which
anything outside a sealed agent composite writes a durable `system.*` slot into its
brain, and a brain opens its own by submitting a mutation that draws that edge. The row
has the familiar shape — `requester: "/os/operator/submit"`, `subject: "*"`,
`scope_match: {"scope_prefix": "/os/orgs", "actions": ["apply"]}` — and it ships
**`enabled: 1`** (R-Policy-Default): an agent that cannot subscribe has no identity, and a
default that refused would make every shipped brain silent until an operator remembered a
row. `code.author.default` beside it stays `enabled: 0`, which is the line this default
does not cross: a brain may open its own identity door on a fresh tree, and it still may
not author code.

**What this row can say, and what it cannot.** It answers **whether** an identity may
subscribe. It cannot express **where** the edge points. Every comparison `policy` makes
is one-sided: a rule field against the matching request field, as an equality, a
wildcard or a path prefix. Nothing here compares two fields of the same request against
each other, so "the edge's target is the subject itself" is not writable as a rule at
any shape, and `*` only asserts presence. That half is checked by
[`submit`](../submit/)'s gate, which is the only place the manifest is readable at all —
`access` never sees it, because a broker answer replaces the body it travelled in. The
gate refuses `subscribe_target_not_self` when the edge ends neither at the requester's
own hive nor at a node the same declaration creates or anything inside one (GH #479,
#561), and
`subscribe_source_not_affinity` when it does not start at an affinity hive,
**before** this broker is asked; this row's `denied` is `subscribe_not_permitted`. That
sentence — *the gate secures the form, the broker answers the capability* — is the
contract between the two templates, and neither half is safe alone: a capability granted
for "any `in_pack` edge" with no form check would let one agent open a door into another
agent's prompt.

#### Narrowing it to one brain

The shipped row grants the capability to every subject under `/os/orgs`. An operator who
means one brain narrows `subject` and, if they want, the prefix, then flips the switch:

```sql
-- from the shipped default …
-- {"rule_id": "affinity.subscribe.default", "subject": "*",
--  "scope_match": {"actions": ["apply"], "scope_prefix": "/os/orgs"}, "enabled": 0}

-- … to one brain, switched on
UPDATE policy
   SET subject     = '/os/orgs/acme/members/alex/talky',
       scope_match = '{"actions": ["apply"], "scope_prefix": "/os/orgs/acme"}',
       enabled     = 1
 WHERE rule_id = 'affinity.subscribe.default';
```

The row is already on, so this `UPDATE` is a **narrowing** rather than a switch: it
replaces `subject: "*"` with one brain and tightens the scope to one organisation.
`enabled: 0` → `enabled: 1` is still the whole switch for the four rows that ship off, and
`examples/vault-pilot` is the shape of a live row for a capability nothing enables by
default. Narrowing `subject` is the operator's lever over *who*; it is still not a lever
over *where*, and it does not need to be — the gate holds the edge to the requester's own
hive whether this row names one subject or all of them.

### `colony.mutate.shell` — the shell is a scope, and no shipped row reached it

Every row above is scoped `/os/orgs`, and for a declaration that grows an organisation, a
member or an assistant that is the right prefix. It is not the right prefix for a
declaration about the **colony itself**. Registering a template class writes to
`/colony/templates`, which belongs to no organisation, so `/os` is the scope root a
composer writes for it — and a submission carrying that scope came back
`requester_not_permitted` from a front door that had no rule to answer it with, while the
identical declaration one level down committed (GH #514).

The fourth row the submitter can ask over is that scope:

| rule_id | capability | requester | subject | enabled | priority | scope_match |
|---|---|---|---|---|---|---|
| `colony.mutate.shell` | `colony.mutate` | `/os/operator/submit` | `*` | **0** | 90 | `{"actions": ["apply"], "scope_prefix": "/os"}` |

**`/os` is the superset, and precedence is what keeps the pair apart.** `scope_prefix` is
a PATH prefix, so `/os` permits `/os`, `/os/orgs` and every address in the colony —
`/os/access` and `/os/operator` included — and still not `/oscar`. The enabled rules for one
capability are read in `priority` **DESC** and the **first match wins**, so at 90 this row
is examined *after* `colony.mutate.default` at 100: a declaration under `/os/orgs` is
answered by the narrow row exactly as before, and this one only ever answers for a scope
the narrow row does not reach. Measured over the shipped script and the shipped seed:

| `resource.scope` | as shipped | with this row switched on |
|---|---|---|
| `/os` | `denied` / `scope_mismatch` | `allowed`, by `colony.mutate.shell` |
| `/os/orgs/acme` | `allowed`, by `colony.mutate.default` | `allowed`, by `colony.mutate.default` |
| `/os/access` | `denied` / `scope_mismatch` | **`allowed`** |
| `/oscar` | `denied` / `scope_mismatch` | `denied` / `scope_mismatch` |

The third line is why this row ships **off** while the two defaults beside it ship on. A
colony that switches it on has granted every shell-level topology change with it — the
broker itself and the submitter that asks it included — and that is a decision an operator
makes rather than one a seed makes for them. It is also why the row is not narrowable by
**where the submission came from**: the requester the broker sees is the literal
`/os/operator/submit` the shell's own edge promotes, and everything that passes the front door
carries `/os/operator/intake` as its `subject`, so *which door* is not an axis a rule can
compare on today.

#### Enabling the shell for the front

The switch is a `seed_rows` declaration (GH #456) — rows enter a running store through the
**mutation door**, under the manifest's digest and into the `mutation_log`, rather than
past it:

```json
[{"scope": "/os",
  "ctx": {},
  "diff": {
    "seed_rows": [
      {"target": "./access/store",
       "table": "policy",
       "rows": [
         {"rule_id": "colony.mutate.shell",
          "capability": "colony.mutate",
          "requester": "/os/operator/submit",
          "subject": "*",
          "scope_match": {"actions": ["apply"], "scope_prefix": "/os"},
          "verdict": "allow",
          "max_ttl_ms": 0,
          "constraints": {},
          "cred_ref": "",
          "priority": 90,
          "enabled": 1,
          "note": "Switched on for this colony: the front registers its own template classes, and a class is registered colony-wide."}
       ]}
    ]
  }}]
```

Three things about that manifest are not obvious, and all three are load-bearing.

**It cannot go through the front door it opens.** Its own scope root is `/os`, so the
submitter would ask exactly the question this row answers, and until the row is on the
answer is still no. It is applied by the **operator** — `meclaw --apply`, or
`POST /colony/mutations`, the road that asks nobody — which is also where it belongs in a
build script: directly after the seed, before the first wish goes through the front.

**`seed_rows` inserts and never updates.** A store's declared tables carry no primary key,
so the operation states *these rows are present*: a row that matches on every column it
names is counted and not written again, and one that differs on any of them is a new row.
Switching a shipped `enabled: 0` on is therefore not an `UPDATE` but this row's **enabled
twin** — the disabled original stays behind as the record of what shipped, and the broker
reads `enabled = 1` and finds exactly one. The operator's other lever is unchanged and is
one line, live and without a restart:

```sql
UPDATE policy SET enabled = 1 WHERE rule_id = 'colony.mutate.shell';
```

What it buys over the manifest is nothing; what it costs is the `mutation_log` entry, so
prefer the manifest wherever a build script is doing the switching.

**A template registration asks a SECOND question.** `add_templates` is the case GH #514
was measured on, and it is executable behaviour arriving with a manifest, so the gate also
asks `code.author` over the same scope root (GH #446). `code.author.default` is scoped
`/os/orgs` and ships off, so a shell-scoped registration with only the row above switched
on moves from `requester_not_permitted` to `code_author_denied` — a different refusal, not
a green light. A front that is meant to extend its own library needs a `code.author` row
at `/os` beside this one, seeded by the same manifest; that is the sharper of the two
grants and it is deliberately not shipped, in any state.

Every comparison in `scope_match` is an equality or a wildcard — except `scope_prefix`,
which is a **path prefix** against `resource.scope`: `/os` permits `/os` and `/os/orgs`
and does **not** permit `/oscar`. It is a new, reserved key rather than prefix semantics
bolted onto `scope`, because reinterpreting `scope` would silently widen every rule that
ever used it, and that is the one change a permission comparator may not make. A rule that
names it needs a `resource.scope` to compare against: without one the answer is
`scope_incomplete`, never a match. And it never enters the grant's frozen scope — a
permission states what is allowed, not where the grant points (R-AC-2).

Rules are examined in `priority` order and the **first match wins**. An explicit `deny` is
worth writing precisely because it reads differently from silence: `denied_by_rule` tells a
caller it found a closed door, `capability_unknown` tells it there is no door there at all.

`require_approval` reports `status: "pending"` and mints **no** grant. The approval lane
itself is not in v1 -- decision B-4 is unruled, and a pending grant nothing can approve
would be a grant that says yes by accident.

### Asking whether, without asking for

A request may carry `"check_only": true`. The verdict is decided exactly as it always was
— same rules, same order, same reason codes — but a matching `allow` answers
`status: "allowed"` with an **empty** `grant_id`, no `expires_at` and no `scope_summary`,
and writes **one** audit row with `outcome: "checked"`. Nothing lands in `grants`, nothing
in `grant_events`.

That is the whole point: a grant nobody redeems is a bearer row with an expiry date that
only `./sweep` ever touches again. A caller who wants to know *whether* an action is
permitted -- `submit` asking whether a manifest may go to the mutation door -- has no
instrument to spend, so it should be handed none. A refused check is the ordinary refusal:
same `reason_code`, `outcome: "denied"`, no grant either way.

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
is **gone** (GH #428). It was deprecated by the sealed delivery, and measuring it showed it had
never worked at all: the emission is a `params`-only body, which the UBF schema refuses, so it
dead-lettered before it was ever logged. A path with no users and no working delivery was not
worth a migration, so it was removed rather than repaired. A config that still carries the key
fails loudly at spawn.

What a deployment has to add instead is **`params.unlock_env`** (GH #427), naming the environment
variable that holds the vault's passphrase. It has to, because a vault inside a sealed hive is
unreachable over the user channel -- a source message cannot address a cell inside a sealed hive,
and everything that can is an edge -- so without it the vault stays locked for its whole life and
every delivery comes back `vault_locked`. The template ships the key UNSET on purpose: "a woken
vault is locked" stays the default truth, and opening one is a decision an operator makes
deliberately. An unlock LANE was rejected outright: it would carry the passphrase through the
`message_log`, which is the failure class sealed delivery exists to end.

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

## A member's own broker (GH #560)

Since `member@1.5.0` this template is an occupant of the **member** level as well
as of the shell, and the two have different jobs. The shell's `access` answers the
submitter's policy questions — may this manifest be applied, may this identity
open its own push lane — and keeps its vault for the keys the OS itself burns (the
builder's brain is an OS-level `llm` cell). A member's `access` answers exactly
one question and holds exactly one kind of thing: the **provider credentials this
person's agents authenticate with**. Both end up with real work, and neither one's
grant table is the other's.

Three things are different about the member-level instance, and all three are
consequences of where it stands:

- **It is reached by v-lanes, not by rim lanes.** The brains that spend its grants
  stand three levels deeper, and the innermost of those levels is sealed. So the
  two edges are drawn at the member's own scope, they name their lane, and the
  surface templates name `./brain` as the connect point (GH #559,
  `templates/member/README.md` § *The credential v-lanes*). The hive path is still
  the address: `in_invoke` in, `ack` out, exactly as here.
- **The parent still owes the `error` drain.** Since `member@1.5.0` the member ships it —
  `./access → .` on `hop.route == 'error'` — and it is the only edge in the
  member's own graph that touches this hive.
- **Its `store` normally takes its grants through the mutation door.** A
  `seed_rows` declaration writes the `grants` and `grant_events` rows the brains
  name (`examples/organism/grow-credentials.json` is the shipped form). Note what
  that does to the shipped seed: `seed_rows` creates the `cell.db` if the store has
  never woken, and a `seed/<table>.jsonl` lands only on a **fresh** database — so
  such a store carries the rows the manifest wrote and not the seven `policy` rows
  this template ships. For a person's broker that is the right outcome: every one
  of those rows is about `colony.mutate`, and a member's broker has no business
  granting that. Seed the `credential.read` rule you *do* want in the same
  declaration.

The vault half is unchanged and stays two operator gestures — `unlock_env` at
birth (there is no params-update operation) and `--vault-add` from stdin with no
colony running. `examples/vault-pilot/README.md` § *Running it* is the whole
sequence in order.

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
(`store/seed/*.jsonl`), so a fresh instance starts from the seed -- the five discretionary rules
`enabled: 0`, the two defaults a colony cannot start without on and narrow -- and an operator
enables or narrows exactly what they mean at the new address. `usage` and `audit` are the
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
- **The `usage` page is bounded** (`params.usage_rows` of `./invoke`, default 500). A `max_invocations`
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

Two lanes, and the line between them is the ruling of
[#138](https://github.com/mmeyerlein/meclaw/issues/138) (R-0904-6): a behaviour
knob is a **param** of the cell that reads it; only the provider lane stays in
`.env`, because a secret in a `config.json` is a secret in the repository.

Since `access@2.5.0` every knob below is a param. Two brokers in one colony can
therefore be bounded apart -- which an environment they share could not say --
and the manifest that grows one sets them with `override_params`, keyed by cell
path:

```json
{"op": "add_nodes", "scope": "/os",
 "nodes": [{"name": "access", "template": "access@2.5.0",
            "override_params": {
              "policy": {"max_ttl_ms": 3600000, "policy_rows": 1000},
              "sweep":  {"sweep_rows": 500},
              "vault":  {"unlock_env": "OS_VAULT_PASSPHRASE"}}}]}
```

The mutation door accepts those keys precisely because they EXIST under `params`
([#294](https://github.com/mmeyerlein/meclaw/issues/294) ruling Q6: an override
names a param the addressed cell has, and a cell with no `params` block has the
empty set). While a knob was a `${...}` token inside `script_inline` it was not a
param key at all, and no override could name it.

A knob left out means the shipped default, and so does `null` -- that is "not
configured", not a crash. Every knob here is a number and is read with `_int`, so
a **blank string** falls back too: there is no number in it to read.

| cell | param | default | meaning |
|---|---|---|---|
| `./clock` | `schedules[0].cron` | `0 */5 * * * *` | 6-field Quartz cron of the TTL sweep, **UTC**. A timer has no top-level `cron` param -- `TimerParams` reads `schedules` -- so an override replaces the whole array |
| `./policy` | `policy_rows` | `200` | page bound of one rule read |
| `./policy` | `max_ttl_ms` | `86400000` | the ceiling no rule can raise |
| `./invoke` | `usage_rows` | `500` | page bound of the quota read |
| `./sweep` | `sweep_rows` | `200` | grants examined per tick |
| `./sweep` | `sweep_event_rows` | `2000` | event page per tick |
| `./vault` | `key_source` | `auto` | where the master key comes from: `auto`, `prompt`, `systemd-cred`, `plainfile`. It names a SOURCE, never key material, which is why it moved with the rest |
| `./vault` | `credential_name` | `vault_key` | the file read under `$CREDENTIALS_DIRECTORY` when `key_source` resolves to `systemd-cred`. A file NAME, not its content |
| `./vault` | `unlock_env` | `null` | **the one setting every deployment makes**: the NAME of the environment variable holding the passphrase. Declared here so a manifest can set it (GH #427); shipped unset, because a woken vault is locked until an operator opens it |

Nothing secret moved onto the params surface, and nothing in this hive stores a
secret in a `config.json`. The passphrase is still an environment variable --
this hive only ever names it -- and the credential material itself lives in the
vault's own encrypted store.

**A standing instance keeps what it was grown with.** Instantiation is a COPY, so
a broker grown before `2.5.0` still carries the old `${ACCESS_*}` / `${VAULT_*}`
tokens in its own `config.json` and still reads them out of the environment.
Nothing here reaches it; what changes is only that a NEW broker will not read
those lines.
