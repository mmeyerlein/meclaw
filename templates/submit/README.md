# `submit@2.0.0`

Two occupants behind one door, and the only reach onto the mutation door in the
whole tree. It asks who may submit; it does not answer it.

## Why it is a hive at all

It routes nothing and hides nothing — `gate` is the whole of the logic; `store`
is the whole of the memory, and it has no address of its own. The store carries
both of the things this hive has to remember across a round trip it does not
control: the **manifest**, parked while the broker is asked, and the **round**,
in flight while the colony applies it. One table, because both rows describe the
same submission and an operator cleaning up after a crash should not have to
know two places. It is a hive
because it stands at a LEVEL BOUNDARY, and a level
derives its own contract from the `params.contract` of its occupants: that is
the form ADR-0013's rule is written in and the form three shipped conformance
tests read. A bare cell here would be an occupant whose accepted lane the
boundary cannot see, which is the mirror image of the defect W7-R5 named — an
emitted lane that dies at the rim.

`params.ports` is `[]`. The hive path is the only address.

## The one edge

`/colony/mutations` cannot be drawn by any mutation, on any scope — the
endpoints a mutation may draw are `/colony/graph` and `/colony/ledger`, and that
list is enumerated rather than prefixed. The edge that carries a submission to
the door therefore lives in the **birth topology** and nowhere else.

Because it lives in exactly one place, the audit trail lives in exactly one
place too. That is the whole architecture: the builder next door drafts and has
no such edge, so "the builder never applies" is a missing edge rather than a
promise, and a missing edge can only be missing **between two nodes**. That is
why drafting and submitting are two occupants of the OS level and not one.

## Phases, recognised positively

| Phase | Recognised by | What it does |
|---|---|---|
| A — a submission | the `manifest` slot carries the ordered **list** | digest and identity, then `[park, ask]` — the manifest into `store` under its digest, and ONE question on `ask` |
| `in_verdict` | `hop.route` says so, re-stamped by the shell | `allowed` → un-parks by digest; anything else → deletes the parked row and refuses |
| `parked` | a store answer whose promoted `context.sub_phase` is `parked` | `[forget the park, remember the flight, submit]` — ONE emission on `mutate`, stamped with its attribution |
| B — the colony's answer | the `manifest` slot carries an **object** with an `outcome` | renders **nothing**: it asks the store whose round this was, and the receipt facts ride in the `carry` |
| `pop` | a store answer whose promoted `context.sub_phase` is `pop` | deletes the row and writes the receipt, stamped with the row's `tool_call_id` and digest |
| `written` | a store answer whose `context.sub_phase` is `written` | nothing at all |

The answer arrives here **directly**: the substrate stamps `reply_to` on every
cell emission, so a reply from `/colony` reaches the cell that emitted even
though it begins a fresh trace with no correlation and no context.

Every phase is recognised by what the message CARRIES, never by what it lacks.
"Anything that is not a fresh submission" would read an error reply as a new
submission and re-emit it — the reply-to-fallback loop this rule is named after.
The store's answers carry `context.sub_origin == 'gate'`, promoted by the hive's
own edge; the key space is deliberately the submitter's own, because the broker
overwrites `ac_*` on its internal edges.

## What the round trip costs, and what it no longer costs

A `/colony` reply begins a **fresh trace**. `emit_reply_or_done` builds a bare
message — a body and a target, no headers, no context, no `reply_to`. That is
not a defect to be repaired in the colony: the function serves seven endpoints,
and "a virtual endpoint answers into a fresh trace" is the property of that
surface, not an accident of it. What it means is that a cell which wants to know
something across the round trip has to **remember** it — which is what the
substrate asks of every cell.

So this hive remembers. `store` holds one row per submission in flight — the
call id, the digest, the requester, the time — written in the same breath as the
emission onto `mutate`, and popped oldest-first when the answer comes back.
The receipt therefore carries the `tool_call_id` of the call that asked for it,
and the `manifest_sha256` of the manifest it answers, and a fan-in waiting on
that id closes on it.

Three properties hold it up, and each is measured rather than asserted:

- **No refusal writes a flight row.** A row without an answer coming would shift
  every following correlation by one — worse than the empty id it was meant to
  heal. The flight row is written where the submission is, one array, one
  routing pass; a refused verdict deletes the parked row and writes no flight
  row at all.
- **A missing row never blocks a receipt.** A restart, an `--apply` from
  outside, a store error: the correlation is lost, the fact is not. The answer
  goes out with an empty id rather than not at all — a lost receipt is the one
  exit there is no recovery from, which is why this hive declares a required
  drain.
- **The pop is FIFO** — ordered by a fixed-width microsecond `at` with the row
  id as tiebreaker, because the column is ordered as TEXT and a tie is a receipt
  handed to the wrong round.

Two limits stay, and are limits rather than oversights:

- **One submission in flight at a time.** The door is serial and so is this
  cell, but the gate emits the next round's `select` as soon as that round's
  answer arrives — which can be before the store has run the previous round's
  `delete`. Both reads then return the same oldest row. Closing that window
  needs a claim inside the read itself, and `store` has no operation that
  removes what it returns (`delete` takes no `order_by`/`limit`, and a bundle's
  arguments are fixed before its first operation runs). Measured in
  `gh438_two_submissions_do_not_swap_receipts`, which says so in its header.
- **The per-generation discriminator on the way down cannot be *required*.** The
  row carries no generation, and it should not: whoever needs one stamps it into
  the `ctx` of their declaration, where it reaches the `mutation_log`.

The draft round is unaffected by any of this: it never crosses `/colony`.

## The digest, checked before anything else

The manifest travelled through a chat: a human read the draft, and a model
repeated it in the second tool call. The digest is taken over the canonical
bytes of the declaration list and is checked here, so a manifest whose bytes
changed on the way is refused **by name** rather than applied by luck.

The helper is the same in three shipped scripts, byte for byte, between the
markers `# --8<-- digest-helper` and `# --8<-- end`, and
`gh425_the_digest_is_one_definition` compares them: a copy that drifts turns the
integrity check into a coin flip that always says no.

## Identity comes off the envelope

`envelope.reply_to`, stamped by the substrate. Never a field in the body: a body
that names itself is a claim, and a claim is not an identity.

The requester and the digest are stamped into the `ctx` of **every** entry —
after the digest check and after the verdict, never before either. The manifest form carries no
manifest-wide `ctx` (each entry is byte-for-byte one single-form body), so an
attribution written at the top level would reach no `mutation_log` row at all.

## Where the policy lives

**Not here.** Ruling R6 says who may build is an `access` policy question, and since GH #435 it is
literally true: this cell holds no rows, no verdicts and no comparator. It asks.

The question is put in the **check-only** form — a verdict and no grant. A grant
would be an instrument, and nobody here would ever spend it: redeeming a grant
at `access` means `./invoke` reaching a connector, and the "connector" would
have to be the mutation door. That would be a **second** edge onto
`/colony/mutations`, and the sentence at the top of this file would stop being
true. Check-only is not the cheap option; the full lane is the wrong one.

**The requester is not the fragende.** `access` reads `requester` off the
**edge** and never out of a body (R-AC-1), and the edge says `/os/submit` — this
hive, asking. The identity the substrate stamped on the submission travels as
`subject`, the axis the broker already has for exactly this case and the one that
is caller-written by design. The rule then reads *submit may `colony.mutate` **on
behalf of** subject S under prefix P*, and the delegation stands visible in a row
instead of hidden in a script.

**One question, over the scope ROOT.** The root is the longest common path
*segment* prefix of every declaration's scope. If the root lies under a permitted
prefix, every declaration lies under it, because every declaration lies under the
root — so one question is not an approximation of N, it is a proof of them. A
manifest that straddles two branches asks about their join, which is stricter and
never more permissive. Segments, not characters: `/os` and `/oscar` join at `/`.
A declaration with no absolute scope widens the question to `/` and lets the
broker refuse it.

The shipped rule is `colony.mutate.default` in `access`'s policy seed, and it
ships `enabled: 0`. A fresh colony submits nothing — the same discipline the
empty `params.policy` used to carry, now in the place that decides.

### The honest price

A colony whose broker does not answer submits nothing **at all** — including a
repair of itself. That is a real cost and it is not hidden here.

The way out is not this cell. `POST /colony/mutations` and `meclaw --apply` do
not go through the broker and are not going to: the operator door and the agent
door are different doors on purpose, and an operator locked out by a policy row
would be a colony with no way back.

## `error_code`

| Code | When |
|---|---|
| `manifest_missing` | no ordered list to submit |
| `manifest_digest_mismatch` | the bytes are not the ones the digest was drawn over |
| `requester_unknown` | no `reply_to` on the envelope — nobody to attribute this to |
| `requester_not_permitted` | the broker refused this submission — the same string this template has always used, so a caller that greps for it keeps working |
| `submission_check_failed` | the broker did not answer a readable verdict, or the parked manifest was gone when the verdict arrived. Nothing was submitted, and that is said rather than guessed |

On phase B the colony's own code is passed through **verbatim** — no new string
is minted here.

## What it is not

- **Not a builder.** It composes nothing and can invent nothing: everything it
  emits was handed to it by somebody whose identity the substrate stamped.
- **Not a validator.** The colony validates. This cell refuses early where
  refusing early is cheaper than refusing at position k.
- **Not transactional.** What committed stays committed. The receipt says where
  it stopped so the rest can be sent again.
