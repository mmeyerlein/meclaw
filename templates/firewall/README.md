# `firewall@2.0.3`

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

**And one refusal that is not a rule.** If the `rules` store does not answer the read
that decides the verdict -- a timeout, a bad column, a table that is not there -- the
screen rejects the turn with `reject_reason` `store_refused` and `rule_id`
`store-refused`, carrying the store's own `error_code` in `hop.store_error` and the
refused op in `hop.store_operation`
([#343](https://github.com/mmeyerlein/meclaw/issues/343)). It is the same fail-closed
reflex as rule 2, one level down: before that guard existed, an unanswered rule read
left the screen with **no** blocklist, **no** allowlist and **no** pattern rule and it
walked on, and an unanswered rate read counted zero arrivals and emitted `pass`. A
firewall whose store times out must refuse the turn, not wave it through.

**But only the two reads that still owe a verdict.** The screen makes three store calls,
and the third is not a question: the arrival `mark` of a passed turn is written
fire-and-forget, in the **same** emission as the `pass` itself. By the time a refusal of
that insert comes back, the parent has already been told what this turn was -- and
`pass`/`reject` is exactly one of two lanes, so a second verdict would leave it with no
way to pick. A refused `mark` therefore emits **nothing**; it writes one line to stderr
(`firewall/screen: arrival mark refused (<code>), rate window undercounts by one`) and
ends there. The cost is stated rather than hidden: that arrival is not booked, so this
turn did not spend its rate slot and the window is short by one -- a bounded fail-open in
the rate dimension alone, for a turn every other rule has already passed.

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

## Lanes and wiring

```json
[
  { "from": "./surface", "to": "./firewall",
    "modifier": {"set_hop": {"route": "'in_turn'"},
                 "set_context": {"channel": "hop.chat_id", "user_id": "hop.user_id"}} },

  { "from": "./firewall", "to": "./intake",
    "condition": "has(hop.route) && hop.route == 'pass'" },

  { "from": "./firewall", "to": "./drain",
    "condition": "has(hop.route) && hop.route == 'reject'" }
]
```

Every endpoint is the firewall HIVE (`params.ports` is empty): `in_turn` in, `pass` and
`reject` out. Which cell screens the turn is this template's business and may change.

| edge | job |
|---|---|
| ingress | names the lane (`hop.route == 'in_turn'`) and promotes **both** identity dimensions. **Without `context.channel` every surface shares the bucket `default` and is rate-limited as one.** Without `context.user_id` the second dimension is the empty string, so every `field: "user_id"` rule is decided against nothing: an `allow` row on that dimension then rejects every turn (`sender_not_allowed`, `allowlist:user_id`) and a `reject` row never fires. Both keys are declared on the lane in `params.contract` |
| pass | the only edge into the agent. `delete_context` drops the parked copy of the turn. |
| reject | the loud lane. `hop.reject_reason` + `hop.rule_id` say what happened; what the parent does with it — drain, log, refuse politely, ban — is the parent's decision. One reason is not a rule at all: `store_refused` (`rule_id` `store-refused`) means the `rules` store did not answer the read that decides the verdict, and then `hop.store_error` carries its `error_code` and `hop.store_operation` the refused op. A refused arrival `mark` does **not** travel here -- that turn already left on `pass`, and one turn gets one verdict; see the rule catalogue. |

**Both exits clear the firewall's own context keys, and the hive does it itself now.**
`context.fw_body` holds a full copy of the turn (that is how a stateless cell carries it
across the store hops); left in place it rides along downstream and doubles the payload of
every later hop. Until GH #228 remembering that was the caller's job; the `pass` and
`reject` exits carry the `delete_context` inside, so it can no longer be forgotten.

The hive's two internal edges (`./screen ⇄ ./rules`) ship inside `config.json` and need
no wiring.

**The hive path is the port contract, and `./screen` is not an address at all.** Since the
seal (GH #228) `params.ports` is empty, so an `add_edges` naming `./firewall/screen` is
refused with `hive_port_boundary`; the working colonies under
[`../../examples/`](../../examples/) wire `./firewall` and let the `in_turn` lane pick the
cell. What is behind the door — one screen cell, two, a different name — may be replaced
in a version bump. The lane names may not: dropping or renaming `in_turn`, `pass` or
`reject` is a breaking change to every parent that wired it, and it gets a CHANGELOG
Breaking entry and a new major version, not a patch.

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
