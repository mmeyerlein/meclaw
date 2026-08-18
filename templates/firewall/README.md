# `firewall@1.0.0`

Deterministic screening on an ingress channel, drawn as topology. One `code` cell
(`screen`) plus one `store` (`rules`) sit between the surface and the agent: every
inbound turn is measured, and it leaves on exactly one of two lanes -- **`pass`** with
the body byte-identical, or **`reject`** naming the reason *and the rule that fired*.

**Nothing here asks a model, and nothing here can.** Every verdict is a comparison, a
character count or a clock. That is the point of GH #36: enforcement is code, only
phrasing is agentic -- and phrasing is not this hive's business.

The pattern is not new, only generalised. Egon's intake carries the Telegram allowlist
today as a literal baked into a script (`ALLOWED = "${TELEGRAM_ALLOWED_USER_ID}"`,
sender mismatch → silent drop). This template turns that one line into rows: auditable,
hot-updatable, visible in the tree, and **loud** when it fires.

## What it delivers

- **A blocked turn never reaches the agent.** The `pass` lane is the only edge into the
  intake; a rejected turn leaves on a different lane entirely.
- **Every match is logged with the rule that fired** — `hop.rule_id` on the reject lane,
  next to `hop.reject_reason`.
- **The rule set is editable without touching cell code** — rows in `rules`, reloaded on
  every single turn. An `update ... set enabled = 1` is a live policy change.
- **A passed turn is byte-identical.** The firewall is a gate, not a rewriter.

## The cells

| path | type | what it holds |
|---|---|---|
| `./screen` | `code` | the verdict. No state, no `cell.db`, no counter — everything it needs rides on the hop or comes out of the store. |
| `./rules` | `store` | `rules` (the policy) and `arrivals` (the stamped trace the rate window counts). |

## The rule catalogue — order matters, first match decides

| # | rule | driven by | `reject_reason` | `rule_id` |
|---|---|---|---|---|
| 1 | **size cap** | `FIREWALL_MAX_CHARS` | `oversize` | `size-cap` |
| 2 | **unreadable rule row** | the `rules` table itself | `rules_unreadable` | the offending row |
| 3 | **sender blocklist** | `kind=sender, action=reject` | `sender_denied` | that row |
| 4 | **sender allowlist** | `kind=sender, action=allow` | `sender_not_allowed` | `allowlist:<field>` |
| 5 | **pattern blocklist** | `kind=substring` / `kind=prefix` | `pattern_blocked` | that row |
| 6 | **rate limit** | `FIREWALL_RATE_MAX` / `_WINDOW_MS` | `rate_limited` | `rate-limit` |

Three of those positions are arguments, not taste:

- **The size cap runs first** because it is the only rule that needs no rule row, and
  because everything past it rides through the hive *parked on a header*. A screen that
  parks an unbounded body before deciding is itself the resource risk it was put there
  to prevent.
- **An unreadable rule row rejects the turn** (fail closed) instead of being skipped. A
  firewall that silently ignores a rule it cannot parse is a hole with a typo in it; the
  reject names the row, so the operator sees exactly which one.
- **The rate limit runs last** because it is the only rule that *consumes* budget. A turn
  that a cheaper rule refuses must never spend a slot — and a rejected turn is never
  booked into `arrivals`.

Within one class, `order_by rule_id asc` decides: two matching deny rows, the
lexicographically first `rule_id` is the one reported.

### The `rules` table

| column | meaning |
|---|---|
| `rule_id` | stable id; this is what lands on `hop.rule_id` |
| `kind` | `sender` \| `substring` \| `prefix` |
| `field` | `channel` \| `user_id` for `sender` rules; `text` for pattern rules |
| `value` | the literal — an exact sender value, or the substring/prefix to look for |
| `action` | `allow` \| `reject` (pattern rules are `reject` only) |
| `enabled` | `0`/`1`; **disabling is the retraction** — nothing is deleted |
| `note` | free text for the operator; the screen never reads it |

**An empty allowlist admits everything, per dimension.** With no enabled `allow` row for
`channel`, the channel dimension is unconstrained; the same holds for `user_id`
independently. A dimension *with* allow rows admits only the values they name. So a
channel is closed by adding one allow row, not by adding a deny row for everyone else.

**Pattern rules are literals.** Both sides are case-folded once; `substring` searches the
turn's text, `prefix` anchors at its beginning. The turn's text is the concatenation of
every `text` field in `messages[]`, in order. No regex engine enters this cell — that is
a deliberate closed door, not a missing feature.

The shipped `seed/rules.jsonl` carries five example rows, **all `enabled: 0`**: the
row-driven rules ship inert (an instantiated firewall must not brick the tree it is
dropped into), while the two arithmetic rules are live from the first turn.

## Knobs

**Env knobs are an experimental surface.** Until this template's knobs move onto the `params`
block of the cells that read them, their names carry no compatibility promise and may change in
any `0.x` release; provider credentials keep living in `.env` either way. The migration is
tracked in [#138](https://github.com/mmeyerlein/meclaw/issues/138), with the
`collector@1.2.0` migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) as the
reference pattern.

| variable | default | effect |
|---|---|---|
| `FIREWALL_MAX_CHARS` | `16000` | size cap of one inbound turn (total characters over `messages[].text`) |
| `FIREWALL_RATE_MAX` | `30` | turns one channel may pass per window; `0` closes the channel |
| `FIREWALL_RATE_WINDOW_MS` | `60000` | width of the rate window in milliseconds |

They are substituted into the script at instantiation (a `code` cell never sees its own
`params`). A non-numeric or unset value falls back to the default rather than crashing.

## Ports and wiring

```json
[
  { "from": "./surface", "to": "./firewall/screen",
    "modifier": {"set_hop": {"route": "'in_turn'"},
                 "set_context": {"channel": "hop.chat_id", "user_id": "hop.user_id"}} },

  { "from": "./firewall/screen", "to": "./intake",
    "condition": "has(hop.route) && hop.route == 'pass'",
    "modifier": {"delete_context": ["fw_body", "fw_now", "fw_phase", "store_origin"]} },

  { "from": "./firewall/screen", "to": "./drain",
    "condition": "has(hop.route) && hop.route == 'reject'",
    "modifier": {"delete_context": ["fw_body", "fw_now", "fw_phase", "store_origin"]} }
]
```

| edge | job |
|---|---|
| ingress | names the lane (`hop.route == 'in_turn'`) and promotes the identity. **Without `context.channel` every surface shares the bucket `default` and is rate-limited as one.** |
| pass | the only edge into the agent. `delete_context` drops the parked copy of the turn. |
| reject | the loud lane. `hop.reject_reason` + `hop.rule_id` say what happened; what the parent does with it — drain, log, refuse politely, ban — is the parent's decision. |

**Both exits must clear the firewall's context keys.** `context.fw_body` holds a full
copy of the turn (that is how a stateless cell carries it across the store hops); left in
place it rides along downstream and doubles the payload of every later hop.

The hive's two internal edges (`./screen ⇄ ./rules`) ship inside `config.json` and need
no wiring.

**`./screen` is the port contract.** It is a stable **address**, not implementation
detail that happens to be reachable: the working colonies under
[`../../examples/`](../../examples/) wire `./firewall/screen` literally, and so does
anything built from this template. The rules cell behind it may be renamed, split or
replaced in a version bump; `./screen` may not — moving it is a breaking change to every
parent that wired it, and it gets a CHANGELOG Breaking entry and a new major version, not
a patch.

## The clock is a seam

`hop.recorded_at` (optional, fixed-width `%Y-%m-%dT%H:%M:%S.%fZ`) stamps a turn from
outside; unset, the screen stamps it itself. Every later hop of the same turn reads that
one stamp back out of `context.fw_now`, so **one turn has exactly one time** — which is
what makes the rate window reproducible from stamped inputs alone, in a test as much as
in a replay.

## Known limits

- **`arrivals` is append-only** and grows with traffic. Trimming it is an operator's
  `delete` with `recorded_at lt <cutoff>`, not a rule and not a background job.
- **The rate bucket is the channel**, not the user. A surface that reports no channel
  runs on one shared bucket.
- **Three store hops per passed turn** (rules, rate window, arrival mark). That is the
  price of a rule set that is data rather than code; the two arithmetic rules that can
  reject *without* a store hop (size cap) run first for exactly that reason.
- **The screen never speaks outbound.** Telling a sender they were refused needs a cell
  on the reject lane that does.
