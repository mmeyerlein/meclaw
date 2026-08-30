# `firewall@2.2.0`

Deterministic screening on an ingress channel, drawn as topology. One `code` cell
(`screen`) plus one `store` (`rules`) sit between the surface and the agent: every
inbound turn is measured, and it ends on exactly one of two lanes -- **`pass`** with
the body byte-identical, or **`reject`** naming the reason *and the rule that fired*.

Since 2.2.0 that sentence has one more word in it: **ends**. A turn a `hold` row parks
does not leave here at once -- it waits in a pile until a person answers, and then it
ends on one of the same two lanes (§ *A turn that waits for a person*). And above every
row of the table stands a layer that is not a row at all: the **hardline** (§ *The
hardline layer*), consulted first, able to say only *reject*, and unreachable by any
`update` an operator or an attacker could write.

Two more cells stand beside the screen and screen nothing. Since 2.1.0 `./porter`
carries the rule table **out** of this hive as a versioned document and takes one
**back** into a hive that is already running, so a colony can be reborn with the screen
the old one ended with instead of an open gate nobody notices (§ *Carrying a screening
policy to another colony*). Since 2.2.0 `./warden` holds the parked turns.

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
- **Some refusals are not rows, and cannot be turned off.** The hardline layer lives in
  this template's code. Emptying the rule table does not open it; changing it is a
  version bump and a CHANGELOG entry.
- **Every match is logged with the rule that fired** — `hop.rule_id` on the reject lane,
  next to `hop.reject_reason`.
- **The rule set is editable without touching cell code** — rows in `rules`, reloaded on
  every single turn. An `update ... set enabled = 1` is a live policy change.
- **A passed turn is byte-identical.** The firewall is a gate, not a rewriter --
  including a turn released out of the hold pile hours later.
- **Every turn ends on exactly one of `pass` and `reject`.** A held one just ends later.
  A hold that nobody answers expires and leaves a receipt; there is no parking bay.

## The cells

| path | type | what it holds |
|---|---|---|
| `./screen` | `code` | the verdict. No state, no `cell.db`, no counter — everything it needs rides on the hop or comes out of the store. |
| `./rules` | `store` | `rules` (the policy), `arrivals` (the stamped trace the rate window counts) and, since 2.1.0, `port_scratch` (the transfer lane's notepad — written and read by `./porter` alone, never by the screen, and it never travels). |
| `./porter` | `code` | the transfer lane, and no verdict at all: it walks `rules` for an `in_export` and writes one part back on `in_import`. Stateless like the screen, which is exactly why the notepad it needs is a table in `./rules`. |
| `./warden` | `code` | the custody of a held turn, and no verdict either: it writes the turn down whole, announces it, and gives it back on `pass` or ends it on `reject` when a person answers or the timeout runs out. |

Four cells, and only the first two decide anything. The porter is reached by the hive's
own `in_export` / `in_import` lanes and never by a turn; the warden is reached by the
screen's own `hold` route and by `in_release` / `in_sweep`.

## The rule catalogue — order matters, first match decides

| # | rule | driven by | `reject_reason` | `rule_id` |
|---|---|---|---|---|
| H | **the hardline** | this template's code | `hardline_blocked` | `hardline:<name>` |
| 1 | **size cap** | `FIREWALL_MAX_CHARS` | `oversize` | `size-cap` |
| 2 | **unreadable rule row** | the `rules` table itself | — (the row is skipped; a receipt names it) | the offending row |
| 3 | **sender blocklist** | `kind=sender, action=reject` | `sender_denied` | that row |
| 4 | **sender allowlist** | `kind=sender, action=allow` | `sender_not_allowed` | `allowlist:<field>` |
| 5 | **pattern blocklist** | `kind=substring` / `prefix` / `suffix` / `glob` | `pattern_blocked` | that row |
| 6 | **rate limit** | `FIREWALL_RATE_MAX` / `_WINDOW_MS` | `rate_limited` | `rate-limit` |
| 7 | **hold** | `kind=sender` / a pattern kind, `action=hold` | — (the turn is parked) | that row |

**And one refusal that is not a rule.** If the `rules` store does not answer the read
that decides the verdict -- a timeout, a bad column, a table that is not there -- the
screen rejects the turn with `reject_reason` `store_refused` and `rule_id`
`store-refused`, carrying the store's own `error_code` in `hop.store_error` and the
refused op in `hop.store_operation`
([#343](https://github.com/mmeyerlein/meclaw/issues/343)). This is where fail-closed
still lives: a table the screen cannot READ AT ALL is a screen that knows nothing, which
is a different case from one unreadable ROW (§ *A row the screen cannot read*). Before
that guard existed, an unanswered rule read
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

Four of those positions are arguments, not taste:

- **The hardline runs ahead of all of them**, because it is not one of them. It is
  consulted before the row-driven catalogue and before the size cap, since one of the
  two hardlines *is* the ceiling that cap can never be configured above.
- **The size cap runs first** because it is the only rule that needs no rule row, and
  because everything past it rides through the hive *parked on a header*. A screen that
  parks an unbounded body before deciding is itself the resource risk it was put there
  to prevent.
- **An unreadable rule row is skipped and announced**, and it does not end the turn. The
  old reading — fail closed, a firewall that silently ignores a rule it cannot parse is a
  hole with a typo in it — was right about the noise and wrong about the blast radius:
  one approved manifest with three wrong *values* closed a member's whole turn lane
  ([#506](https://github.com/mmeyerlein/meclaw/issues/506)). The row now enforces
  nothing, a bodiless receipt on the reject lane names it and its fault on every turn,
  and fail-closed stays where a row cannot reach it — the hardline, and a rules table
  that does not answer at all (§ *A row the screen cannot read*).
- **The rate limit runs last of the refusing rules** because it is the only rule that
  *consumes* budget. A turn that a cheaper rule refuses must never spend a slot — and a
  rejected turn is never booked into `arrivals`.
- **`hold` is asked last of all** because it is the only verdict that is not final.
  Everything that can end the turn outright ends it first, and only a turn nothing
  refused is parked. A held turn *does* book its arrival: a parked turn occupies a slot
  in the pile and cost the screen its whole walk, so a hold that were free would be a
  flood channel with a rule row for a door.

Within one class, `order_by rule_id asc` decides: two matching deny rows, the
lexicographically first `rule_id` is the one reported.

### The hardline layer

Everything above is a row, and every row is editable at runtime by design — an
`update ... set enabled = 1` is a live policy change, and that is a feature. The
inverse is delivered just as reliably: an `update ... set enabled = 0`, or a `delete`,
turns any of it off.

That is correct for policy. It is wrong for the small set of refusals that exist
precisely because the operator — or an agent acting for them, or an attacker who reached
the store — must **not** be able to lift them. Those live in this template's **code**,
they are consulted before the row-driven catalogue, and they can only ever say `reject`.
**A hardline never grants**: a layer that could allow would be an authority mechanism,
and this is not one.

| `rule_id` | what it refuses | why no row may lift it |
|---|---|---|
| `hardline:body-ceiling` | a turn above 262 144 characters | `FIREWALL_MAX_CHARS` is a knob, and a knob set to a billion turns the screen itself into the resource risk it stands in front of. The ceiling bounds the knob. |
| `hardline:invisible-format` | a turn carrying an invisible or direction-**overriding** codepoint, or a control character other than tab, newline and carriage return | one zero-width space inside a forbidden literal defeats *every* pattern row at once. A row that could switch this off would switch off the effectiveness of the whole table, which is the class this layer exists for. |
| `hardline:hold-ceiling` | a turn that would push the hold pile past 1024 | `FIREWALL_HOLD_MAX` may only **lower** that number. An unbounded pile is an outage of the channel wearing the mask of a queue. |

Two exclusions in the second one are deliberate and are not oversights: **ZWNJ (U+200C)
and ZWJ (U+200D)** carry emoji sequences and Indic/Arabic joining, and the directional
**marks** (U+200E, U+200F, U+061C) appear in ordinary right-to-left text. A hardline that
refuses a family emoji is a hardline an operator disables, and a hardline an operator
disables is a row with extra steps.

Three consequences, and they are the point rather than a side effect:

- **A hardline cannot be disabled with an `update`, because it is not a row.**
- **Changing one is a template version bump and a CHANGELOG entry**, not an insert.
- **The reject lane says *which* hardline fired**, on the same `hop.rule_id` a rule row
  lands on, so an operator can tell "policy said no" from "the substrate said no".

**The scope is the hive** (ruled with [#449](https://github.com/mmeyerlein/meclaw/issues/449)).
Three were on the table. *Colony* matches where write authority already lives but would
make this template depend on colony-level configuration it cannot carry itself. *Member*
is natural for "this agent may never be told to do X" and awkward for "this ingress
channel never accepts Y", which is the only kind of sentence a screen can form. *Hive*
is self-contained, portable, and versioned with the template — which is what makes
"changing one is a version bump" literally true rather than a convention someone has to
keep. The named weakness of hive scope — an operator who can edit the tree could replace
the hive — is not what this layer defends against: an operator with mutation authority
can remove the firewall outright. The layer stands against the **rows** path, which is
the one that is editable by design.

### Precedence, said once and plainly

```
hardline  >  deny  >  allow  >  hold
```

**Deny beats allow, first match per rule class decides, and specificity does not
matter** — and above all three stands the hardline, which no row of any kind reaches.
The rule classes are consulted in the numbered order above and the
first one that fires ends the turn; inside a class the lowest `rule_id` wins.
`hold` sits at the bottom for the reason it is asked last: it is the only verdict that
does not end the turn, so every verdict that *can* end it is asked first. A hold is not
a weak deny and a hardline is not a very broad deny — they are different layers, and
that is why the order can be stated in one line.
Nothing about a rule makes it outrank another rule of its class — not a longer
literal, not a narrower pattern, not a row that names two dimensions where its
neighbour names one. A combined `channel + user_id` row is not "more specific"
in any sense the screen can see; it is simply a row whose condition is an AND.

The consequence is the part worth stating: **there is no way to carve an allow
exception out of a broad deny.** A deny row for a channel refuses every sender
on it, and an allow row naming one of them — even one naming that sender *and*
that channel — does not rescue them, because RULE 3 has already emitted the
verdict before RULE 4 is read. To admit one sender on an otherwise closed
channel, narrow the deny row or drop it and close the dimension with allow rows
instead. That is the same shape Claude Code's own permission rules use (`deny`
before `allow`, first match, specificity irrelevant), and it is chosen for the
same reason: a precedence a reader can hold in their head is worth more than one
that is cleverer. A hardline is the one thing in the picture that is *not* a rule, and
that is exactly why it is drawn above the line rather than as a very early row.

### The `rules` table

| column | meaning |
|---|---|
| `rule_id` | stable id; this is what lands on `hop.rule_id` |
| `kind` | `sender` \| `substring` \| `prefix` \| `suffix` \| `glob` |
| `field` | `channel` \| `user_id` \| `match` for `sender` rules; `text` for pattern rules |
| `value` | the literal — a sender value, the pattern to look for, or (with `field: match`) a JSON object naming several sender dimensions at once |
| `action` | `allow` \| `reject` \| `hold` (a pattern rule may be `reject` or `hold`, never `allow`) |
| `enabled` | `0`/`1`; **disabling is the retraction** — nothing is deleted |
| `note` | free text for the operator; the screen never reads it |

**Three of the seven columns are CLOSED VALUE SETS**, and they are the half of the
declaration nothing but this cell enforces. `seed_rows` checks that a row's keys are
columns the store declares — it has no vocabulary for values, by design
([#456](https://github.com/mmeyerlein/meclaw/issues/456)) — so a row can be permitted,
digested, ledgered and applied and still say nothing this screen understands:

| column | the closed set |
|---|---|
| `kind` | `sender` \| `substring` \| `prefix` \| `suffix` \| `glob` |
| `field` | `channel` \| `user_id` \| `match` on a `sender` row; exactly `text` on a pattern row |
| `action` | `allow` \| `reject` \| `hold` on a `sender` row; `reject` \| `hold` on a pattern row |

There is no `pattern` kind, no `body` field, no `deny` action and no regex anywhere.
Those three are exactly the values one measured, approved manifest guessed
([#506](https://github.com/mmeyerlein/meclaw/issues/506)), which is why they are written
down here and in `template.json` — the surface a composer actually retrieves.

### A row the screen cannot read

**It is skipped, it is announced, and it does not close the lane.** A row whose values
fall outside the vocabulary above — or that carries no `rule_id`, no `value`, or a
`field: match` whose `value` is not a JSON object over `channel`/`user_id` — leaves the
rule set *before rule 3 reads it*. It is in no blocklist, no allowlist, no pattern set
and no hold set: nothing could tell what policy it meant, so it enforces none.

The skip is **loud**. Once per row, on every turn the row survives, the `reject` lane
carries a **receipt**:

```
route=reject  reject_reason=rule_unreadable  rule_id=<the row>  rule_fault=<why>
```

`rule_fault` is a closed set too — `unnamed_row`, `unknown_kind`, `unknown_field`,
`unknown_action`, `empty_value`, `unreadable_match` — and the receipt's body is
**empty**. That is how a drain tells a receipt from a verdict: *a verdict always carries
the turn it is about, a receipt never does.* The turn itself leaves on its own lane in
the same emission, screened by whatever rows were readable.

**Why it stopped refusing the turn.** Until [#506](https://github.com/mmeyerlein/meclaw/issues/506)
an unreadable row rejected the turn — fail closed, name the row. That reflex is right and
it was in the wrong place, and the wrong place was measured on a live colony: one
approved `seed_rows` with every column correct and three values outside the vocabulary
(`kind: "pattern"`, `field: "body"`, `action: "deny"`) closed the **whole turn lane** of
the member behind the screen. A wish that said *"refuse turns that mention a phone
number"* had built *"refuse everything"*, and nothing reported a fault. One wrong field
value in one manifest must not be a denial of service for the agent being screened.

**What still fails closed**, and it is the part that carries the security argument:

- **the hardline** — code, not a row, so no row can make it unreadable
  (§ *The hardline layer*);
- **a rules table that cannot be read at all** — `store_refused`, above. A missing
  answer is not a permissive answer.

A table whose rows are *all* unreadable therefore leaves a screen running on the hardline
and the two arithmetic rules and nothing else. That is an open gate with an alarm on it,
and the alarm is the point: it rings on every turn until someone writes one `update`.
An alarm that rings once is an alarm nobody sees.

**An empty allowlist admits everything, per dimension.** With no enabled `allow` row for
`channel`, the channel dimension is unconstrained; the same holds for `user_id`
independently. A dimension *with* allow rows admits only the values they name. So a
channel is closed by adding one allow row, not by adding a deny row for everyone else.

**One row may require two dimensions.** A `sender` row with `field: match` carries a
JSON object in `value` — `{"channel": "tg:42", "user_id": "7"}` — and matches only when
**every** dimension it names matches. Two separate rows are an OR; this is the AND, and
"this account, in this room" is the shape most real policies have. It rides in the
`value` column the store has always had, so an instantiated firewall takes one with an
`insert` and needs no migration. An `allow` row of this shape constrains every dimension
it names and is satisfied as a whole or not at all: a turn matching one half of the pair
leaves both halves unsatisfied, and the refusal names the first of them
(`allowlist:channel`). A `value` that is not a JSON object over `channel`/`user_id` with
non-empty literals is unreadable like any other malformed row: skipped, and named on the
reject lane with `rule_fault` `unreadable_match`.

**Pattern rules are literals, with four match modes.** `substring` searches the turn's
text, `prefix` anchors at its beginning, `suffix` at its end, and `glob` carries the
`fnmatch` wildcards `*`, `?` and `[...]` over the **whole** text — so a glob without
stars is an equality test, not a search. The turn's text is the concatenation of every
`text` field in `messages[]`, in order. No regex an operator can write enters this cell:
that closed door stays closed, and a glob is a bounded pattern language, not an engine
with a backtracking budget.

**Every comparison runs on one normalised form.** Before any rule is applied — pattern
rules and `sender` rules alike, both sides — the text is NFKC-normalised, every run of
whitespace is collapsed to a single space, the result is trimmed and casefolded. That is
what closes the two evasions a plain lowercase leaves open: a turn written in fullwidth
or ligature codepoints does not contain the ASCII literal a rule names, and neither does
one with a newline or a NO-BREAK SPACE where the rule has a blank. It also means a
channel id differing only in case or padding is the same channel, and that an operator
who pastes a literal with a stray tab into the store wrote the rule they meant to write.
**The size cap is deliberately not normalised**: it counts what arrived, because it is a
resource bound rather than a comparison, and measuring the collapsed form would let a
padded body buy budget it never paid for.

The shipped `seed/rules.jsonl` carries **one live rule** and 8 example rows with
`enabled: 0`. The live one is `block-prompt-injection`: a case-folded, NFKC-normalised
`substring` on the canonical injection opener. It refuses, it never grants, it names no
sender, and no ordinary turn contains it — so a freshly instantiated firewall enforces
something on its first turn instead of shipping as an open gate with a rule table
attached, and still does not brick the tree it was dropped into. Disable it with the
same `update` as any other row if this channel needs to discuss the phrase. The 7
examples stay inert and exist to be read and enabled; the two arithmetic rules (size,
rate) are live from the first turn as they always were, and so is the hardline layer,
which is not a row and cannot be otherwise.

## A turn that waits for a person (#450)

Two lanes answer a turn immediately, and several rules would be better served by an
answer that is not immediate: a turn that is *probably* fine, from a sender nobody has
vouched for yet, on a channel that was opened an hour ago. `hold` is that third verdict.
The turn is parked whole, nothing downstream sees it, and a person decides later whether
it continues on `pass` or ends on `reject`.

**It does not break the two-lane promise, and that is a design constraint rather than a
happy accident.** "Exactly one of two lanes" is what makes the `pass` edge provably the
only route into the agent, and it is why a refused arrival `mark` emits nothing rather
than a second verdict. So `hold` is not a third answer about the turn: it is *no answer
yet*. The turn still ends on exactly one of `pass` and `reject` — later. What the `hold`
lane carries is a **notice**, and the notice is what a person reads.

| what | where |
|---|---|
| the parked turn | one row in `./rules`'s `held` table: body, hop and the context its ingress edge promoted, stored whole and not summarised |
| the notice | the `hold` lane, carrying `hop.hold_id`, `hop.rule_id`, `hop.expires_at`, `hop.held_at` and the turn itself |
| the answer | `in_release` — `hop.hold_id` names one parked turn, `hop.decision` is `release` or `refuse`, `hop.decided_by` is who answered |
| the timeout | `FIREWALL_HOLD_TTL_MS`, and its receipt is `hold_expired` on the `reject` lane |

**The row is written before anyone is told, and the answer is written before the turn
moves.** Both orderings are load-bearing. A notice naming a hold that does not exist is a
lie an operator would act on; a turn delivered before its row says `released` can be
delivered twice, because the release names the hold and nothing else. The release
`update` carries `status: 'held'` in its own where clause, so a second release of one
hold changes no row and produces no second delivery.

**An expired hold can never become a `pass`.** Expiry is measured whenever custody is
touched — a new hold, a release, an `in_sweep` — and a release of an overdue hold flips
the row to `expired` and ends the turn on `reject`. There is no window in which the
timeout has passed and the turn still travels.

**The pile is bounded twice.** `FIREWALL_HOLD_MAX` (default 100) refuses a would-be hold
with `hold_pile_full` rather than dropping it, and the hardline ceiling of 1024 is the
number that knob may only lower. A held turn also books its arrival, so the rate window
bounds how fast the pile can fill in the first place.

**A released turn carries its dimensions back as hop keys.** A cell cannot write a
context map and a `set_context` key is static, so every context key the original ingress
edge promoted returns on the `pass` hop as `ctx_<name>`. The firewall's own exit restores
the two dimensions it owns (`channel`, `user_id`); a parent that promoted more — say
`context.assistant` — reads its own back with

```
"modifier": {"set_context": {
  "assistant": "has(hop.ctx_assistant) ? hop.ctx_assistant : context.assistant"}}
```

on the edge it already draws off `pass`. Guard it with `has()`: an ordinary passed turn
carries no `ctx_*` key, and a CEL modifier that fails skips the whole edge.

**Wiring, all of it opt-in.** `params.required_drains` pairs `in_release` with both
`pass` and `reject`, and `in_sweep` with `reject` — draw the lane and the mutation holds
you to its exits. The `hold` notice lane is deliberately **not** in that list: requiring
it would break every parent that already wired an `in_turn` and never asked for a hold.
A colony that enables a hold row without drawing the notice edge is not silent, though:
the notice becomes a recorded, self-localising `no_route` in the DLQ, and the turn's
expiry receipt still arrives on `reject`, which every parent already drains.

```json
[
  { "from": "./ops", "to": "./firewall",
    "modifier": {"set_hop": {"route": "'in_release'",
                             "hold_id": "hop.hold_id",
                             "decision": "'release'",
                             "decided_by": "hop.decided_by"}} },
  { "from": "./firewall", "to": "./intake",
    "condition": "has(hop.route) && hop.route == 'pass'" },
  { "from": "./firewall", "to": "./drain",
    "condition": "has(hop.route) && hop.route == 'reject'" },
  { "from": "./firewall", "to": "./review",
    "condition": "has(hop.route) && hop.route == 'hold'" }
]
```

The `pass` and `reject` edges the screen already needs are the two drains `in_release`
asks for — one lane, two producers, as with the porter. `in_sweep` needs a producer with
a clock: a `timer` cell in the parent, or an operator. Without one, a hive that went
quiet emits its expiry receipts at the next touch instead of on the minute, which is the
honest limit rather than a hidden one.

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
| `FIREWALL_HOLD_TTL_MS` | `3600000` | how long a parked turn waits for a person before it expires. There is no value that switches the timeout off |
| `FIREWALL_HOLD_MAX` | `100` | how many turns may be parked at once; a turn above it is **rejected**, never dropped. It may only lower the hardline ceiling of 1024, never raise it |

They are substituted into the script at instantiation (a `code` cell never sees its own
`params`). A non-numeric or unset value falls back to the default rather than crashing.

## Lanes and wiring

```json
[
  { "from": "./door", "to": "./firewall",
    "modifier": {"set_hop": {"route": "'in_turn'"},
                 "set_context": {"channel": "hop.chat_id", "user_id": "hop.user_id"}} },

  { "from": "./firewall", "to": "./intake",
    "condition": "has(hop.route) && hop.route == 'pass'" },

  { "from": "./firewall", "to": "./drain",
    "condition": "has(hop.route) && hop.route == 'reject'" }
]
```

Every endpoint is the firewall HIVE (`params.ports` is empty): `in_turn`, `in_export`,
`in_import` and — since 2.2.0 — `in_release` and `in_sweep` in; `pass`, `reject`, `dump`
and `hold` out. Which cell screens the turn is this template's business and may change.
The three hold lanes are opt-in: a colony with no enabled `hold` row never uses them, and
the four edges above are still the whole wiring for one.

| edge | job |
|---|---|
| ingress | names the lane (`hop.route == 'in_turn'`) and promotes the identity dimensions. **Without `context.channel` every surface shares the bucket `default` and is rate-limited as one** — that harm lands on every turn, so `channel` is the one key the lane DECLARES in `params.contract`, and since [#291](https://github.com/mmeyerlein/meclaw/issues/291) that declaration is checked at the mutation. **`context.user_id` is optional and deliberately not declared:** unpromoted it is the empty string, which the screen treats like any other value — with no *enabled* allow row on that dimension nothing is constrained, and a deny row on the empty string cannot exist (a row without a value is not a rule — it is skipped and named on the reject lane). Enable one `allow` row on `user_id` and the calculus flips: every turn without a promoted `user_id` is then rejected (`sender_not_allowed`, `allowlist:user_id`). Promote it whenever this colony has per-user rules |
| pass | the only edge into the agent. `delete_context` drops the parked copy of the turn. Since 2.2.0 it has a second producer: a released turn re-enters here with the body it arrived with, and the context its ingress edge promoted comes back as `hop.ctx_<name>` (see § *A turn that waits for a person*). |
| reject | the loud lane. `hop.reject_reason` + `hop.rule_id` say what happened; what the parent does with it — drain, log, refuse politely, ban — is the parent's decision. One reason is not a rule at all: `store_refused` (`rule_id` `store-refused`) means the `rules` store did not answer the read that decides the verdict, and then `hop.store_error` carries its `error_code` and `hop.store_operation` the refused op. One thing on this lane is not a verdict at all: the `rule_unreadable` **receipt**, which names a row rather than a turn and carries an empty body (§ *A row the screen cannot read*). A refused arrival `mark` does **not** travel here -- that turn already left on `pass`, and one turn gets one verdict; see the rule catalogue. Since 2.1.0 the lane has a **second producer**: `./porter` refuses a transfer it will not carry out and names the case in the same `hop.reject_reason` (no `rule_id` — a refused document broke no rule). One drain takes both; the reason code tells them apart. Since 2.2.0 there is a **third**: `./warden` ends a parked turn here — `hold_refused`, `hold_expired` (also emitted unasked by a sweep), `hold_pile_full`, `hardline_blocked`, `hold_unknown`, `hold_not_pending`. That is what keeps the two-lane promise true through a third verdict. |
| in_export / in_import | the transfer lane. `in_export` demands the whole rule table as a versioned document, `in_import` feeds ONE part of such a document back into a running hive. Both declare an empty `context`: an export is about the hive, not about a round. |
| in_release / in_sweep | the hold lane. `in_release` carries one person's answer about one parked turn (`hop.hold_id`, `hop.decision`, `hop.decided_by`); `in_sweep` carries nothing and expires whatever is due. `params.required_drains` pairs `in_release` with `pass` AND `reject`, and `in_sweep` with `reject` — draw one and the mutation holds you to its exits. |
| hold | the notice that a turn was parked and its row is written: `hop.hold_id`, `hop.rule_id`, `hop.expires_at`, `hop.held_at` and the turn itself. It is the ONE lane of this hive with no required drain, because requiring it would break every parent that wired an `in_turn` and never asked for a hold. Undrawn it is a recorded `no_route` in the DLQ, and the expiry receipt still arrives on `reject`. |
| dump | the transfer lane's output — an export part (`hop.dump_kind == 'export_part'`) or the receipt of an applied one (`'import_receipt'`). **Drain it with a PLAIN `hop.route == 'dump'` test**: an edge that additionally tests `dump_kind` reads as no drain at all under the `required_drains` probe, and the mutation is refused. |

**Both exits clear the firewall's own context keys, and the hive does it itself now.**
`context.fw_body` holds a full copy of the turn (that is how a stateless cell carries it
across the store hops); left in place it rides along downstream and doubles the payload of
every later hop. Until GH #228 remembering that was the caller's job; the `pass` and
`reject` exits carry the `delete_context` inside, so it can no longer be forgotten.

The hive's internal edges ship inside `config.json` and need no wiring: `./screen ⇄
./rules` for the verdict, and since 2.1.0 `. → ./porter` on the two ingress lanes,
`./porter ⇄ ./rules` for the walk, and `./porter → .` on `dump` and on `reject`. Since
2.2.0, `./screen → ./warden` on `hold`, `. → ./warden` on the two hold ingress lanes,
`./warden ⇄ ./rules` for custody, and `./warden → .` on `pass`, `reject` and `hold`.

**The hive path is the port contract, and `./screen` is not an address at all.** Since the
seal (GH #228) `params.ports` is empty, so an `add_edges` naming `./firewall/screen` is
refused with `hive_port_boundary`; the working colonies under
[`../../examples/`](../../examples/) wire `./firewall` and let the `in_turn` lane pick the
cell. What is behind the door — one screen cell, two, a different name — may be replaced
in a version bump. The lane names may not: dropping or renaming `in_turn`, `pass`,
`reject`, `in_export`, `in_import` or `dump` is a breaking change to every parent that
wired it, and it gets a CHANGELOG Breaking entry and a new major version, not a patch.
Adding one is not: `in_release`, `in_sweep` and `hold` arrived in a minor bump precisely
because no parent that ignores them notices they exist — `required_drains` only asks
about a lane an edge actually names.

## The clock is a seam

`hop.recorded_at` (optional, fixed-width `%Y-%m-%dT%H:%M:%S.%fZ`) stamps a turn from
outside; unset, the screen stamps it itself. Every later hop of the same turn reads that
one stamp back out of `context.fw_now`, so **one turn has exactly one time** — which is
what makes the rate window reproducible from stamped inputs alone, in a test as much as
in a replay.

## Carrying a screening policy to another colony (#471)

A firewall's rules **are** its screen. A colony reborn without them is not a colony with
an empty table — it is a member that admits everything the old one refused, and an open
gate emits no event to notice it by. Until 2.1.0 the only way out of this hive was a
hand-written `sqlite3` pipeline reaching around the very `cell.db` boundary
[#160](https://github.com/mmeyerlein/meclaw/issues/160) keeps closed. Two lanes close
that: `in_export` writes the rules out, `in_import` takes them back into a **running** one.

**The document.** One content table means one part, and that part is also the last one —
which is what lets the completeness marker exist at all:

```json
{"format": "meclaw-firewall-export/1", "hive_template": "firewall",
 "export_id": "…", "exported_at": "…",
 "table": "rules", "part": 1, "of": 1, "final": true, "absent": false,
 "key": ["rule_id"], "schema": {"rule_id": "text", "…": "…", "note": "text"},
 "rows": [ {"rule_id": "block-prompt-injection", "…": "…"} ]}
```

`schema` is the store's own declaration, so `{"schema": …}` as line 1 plus one row per
line after it **is** a `rules/seed/rules.jsonl` — birth path and transfer path speak one
format. `final` (mirrored as `hop.export_final == "1"`) is the completeness marker; a
document without it is partial, and nothing else in it would say so. `absent` is not
`rows: []`: an empty table held no rules, an absent one never had the table.

**Wiring, in the same mutation as the ingress.** `params.required_drains` pairs each
ingress lane with both of its exits (`in_export → dump`, `in_export → reject`,
`in_import → dump`, `in_import → reject`) and refuses the mutation unless all of them are
drawn at once — an export that reaches nobody read the whole store for nothing, and an
undrained refusal makes a transfer that did not happen look exactly like one that did.

```json
[
  { "from": "./ops", "to": "./firewall",
    "modifier": {"set_hop": {"route": "'in_export'"}} },
  { "from": "./firewall", "to": "./export-sink",
    "condition": "has(hop.route) && hop.route == 'dump'" },
  { "from": "./firewall", "to": "./drain",
    "condition": "has(hop.route) && hop.route == 'reject'" }
]
```

The `reject` edge the screen already needs is that second drain — one lane, two
producers. The `dump` edge stays a **plain** route test, for the reason under *Lanes and
wiring*: the probe runs the described hop through the real edge evaluator.

**What stays behind.** `arrivals` does not travel: it is the rate window **this**
installation consumed, bound to this installation's channel identities, and a colony that
inherited a full one would refuse turns for traffic it never saw. A screen is its rules,
not the budget somebody else spent. `port_scratch` stays for the reason every notepad
does. A part naming either is refused with `import_unknown_table`.

**Applying the same part twice leaves the same state.** `params.schema` cannot express a
key, so a repeated `insert` would duplicate the row. The importer probes first: it parks
the part and the target's `rule_id` column under one `port_scratch` key, reads both back
in one `select`, and inserts only what is missing. The target wins every collision — an
import never updates, so a rule an operator disabled here is not silently re-enabled by a
document from elsewhere, and "send it again" is the repair for any failure.

**Every refusal is fail-closed and lands on `reject`:** `export_read_failed`,
`import_format`, `import_unknown_table`, `import_schema_drift` (the source declares a
column this store does not have — growing a schema is a template change, not something an
import does silently), `import_probe_failed`, `import_write_failed`. There is no
`missing_audience` here and there is not meant to be: a rule names no audience, it is a
screen in front of the whole member.

**Birth is the other half, and it is a file.** A seed is read **once**, when the `cell.db`
is created, and is inert for ever after: to give a firewall rules before it exists, write
the part as `rules/seed/rules.jsonl` and instantiate. Under a member the export sink has
already filed it there — `<export_dir>/firewall/seed/rules.jsonl` — because
`member/firewall` is a `ref` directly beneath the member, so the member's `in_export`
fans out to this hive's and an import is routed back by `hop.import_hive == 'firewall'`.

Pinned by `crates/meclaw-cells/tests/gh471_the_porters_mirror_their_stores.rs`: it
compares the porter's schema mirror against `rules/config.json` column for column and is
red the moment a column is added to the store and not to the walk.

## Known limits

- **A part is a whole table.** There is no paging: a truncated part would lie about
  being a table, so a rule set that outgrows one message has no shipped answer yet.
- **An import is confirmed by asking.** The receipt on `dump` says how many inserts one
  part dispatched; a write that failed afterwards arrives on `reject` as
  `import_write_failed`. Neither is a transaction — re-applying the document is the
  repair, and it is safe by construction.
- **`arrivals` is append-only** and grows with traffic. Trimming it is an operator's
  `delete` with `recorded_at lt <cutoff>`, not a rule and not a background job.
- **The rate bucket is the channel**, not the user. A surface that reports no channel
  runs on one shared bucket.
- **Three store hops per passed turn** (rules, rate window, arrival mark). That is the
  price of a rule set that is data rather than code; the two arithmetic rules that can
  reject *without* a store hop (size cap) run first for exactly that reason.
- **The screen never speaks outbound.** Telling a sender they were refused needs a cell
  on the reject lane that does. Telling a *person* that a turn is waiting for them needs a
  cell on the `hold` lane, for the same reason and by the same rule.
- **A sweep is somebody else's clock.** This hive has no timer. Expiry is measured
  whenever custody is touched — a hold, a release, an `in_sweep` — so a hive that went
  quiet emits its expiry receipts at the next touch rather than on the minute. Wire
  `in_sweep` to a timer if the receipt has to be punctual. The guarantee that does not
  depend on it: an expired hold can never become a `pass`, because the release checks the
  stamp itself.
- **An expiry receipt names the turn; it does not carry it.** The pile read is five
  columns wide on purpose — a sweep of a thousand overdue holds must not put a thousand
  turns back on the wire. The body stays in the row for an operator to read.
- **A released turn cannot be given back an arbitrary context.** A cell cannot write a
  context map and a `set_context` key is static, so the dimensions come back as
  `hop.ctx_<name>` and the parent's own `pass` edge decides which of them to promote. The
  firewall restores the two it owns and no more.
- **The hold pile does not travel in an export.** A parked turn is a live conversation
  waiting on a person, addressed to a colony that no longer exists; releasing an inherited
  hold would inject a stranger's turn into an agent that never met them. A part naming
  `held` is refused with `import_unknown_table`, like `arrivals` and `port_scratch`.
