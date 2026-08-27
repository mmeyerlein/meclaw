# `submit@1.0.0`

One cell behind one door, and the only reach onto the mutation door in the whole
tree.

## Why it is a hive at all

It routes nothing and hides nothing — `gate` is the only occupant and the whole
of the logic. It is a hive because it stands at a LEVEL BOUNDARY, and a level
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

## Two phases, recognised positively

| Phase | Recognised by | What it does |
|---|---|---|
| A — a submission | the `manifest` slot carries the ordered **list** | digest, identity, policy, then ONE emission on `mutate` |
| B — the colony's answer | the `manifest` slot carries an **object** with an `outcome` | renders the receipt and sends it back down |

The answer arrives here **directly**: the substrate stamps `reply_to` on every
cell emission, so a reply from `/colony` reaches the cell that emitted even
though it begins a fresh trace with no correlation and no context.

Phase B is recognised by what it CARRIES, never by what it lacks. "Anything that
is not a fresh submission" would read an error reply as a new submission and
re-emit it — the reply-to-fallback loop this rule is named after.

## What the round trip costs: the receipt has no correlation

A `/colony` reply begins a **fresh trace**. `emit_reply_or_done` builds a bare
message — a body and a target, no headers, no context, no `reply_to`. Everything
the apply round was correlated by is therefore gone by the time the receipt
travels back down, and this cell keeps no memory between its two phases:

- the `tool_result` that closes the apply round carries an **empty**
  `tool_call_id`, so a fan-in waiting on that id does not close on it;
- the per-generation discriminator on the way down cannot be *required*, so a
  member with two generations would see the receipt reach both.

The draft round is unaffected: it never crosses `/colony`.

The fix is a small store beside this cell, one row per submission in flight
(requester, call id), popped in FIFO order — the mutation door is serial, so
submission order and answer order are the same. That is the same store
[#435](https://github.com/mmeyerlein/meclaw/issues/435) needs for the policy
round trip, doing double duty. Tracked at
[#438](https://github.com/mmeyerlein/meclaw/issues/438), and asserted rather
than hidden by the scenario case `I1`.

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
after the digest check, never before it. The manifest form carries no
manifest-wide `ctx` (each entry is byte-for-byte one single-form body), so an
attribution written at the top level would reach no `mutation_log` row at all.

## Policy as rows

`params.policy` is a list, not logic. First matching row wins, and no row means
no.

| Field | Meaning |
|---|---|
| `requester_prefix` | a path prefix the requester's own path must start with |
| `verdict` | `allow` or `deny` |
| `scopes` | the scope prefixes this requester may address |

A permitted scope is a **path** prefix: `/os` permits `/os` and `/os/orgs`, and
does not permit `/oscar`. The check runs over every declaration before anything
is emitted — a manifest rolls forward with no rollback, so half a submission is
worse than none.

**Shipped empty.** A fresh instance submits nothing, the same discipline as
`access`'s seed with `enabled: 0`. The value is a param, so an instance supplies
it at instantiation (`SUBMIT_POLICY`) or per `override_params`; a list and a
JSON string are both accepted.

## Where the policy will live

Not here. R6 says "wer bauen darf = access-Policy", and routing the decision
through the capability broker is the better shape.

It is not the first step, for a mechanical reason rather than a matter of taste:
`access` answers with a `tool_result` that REPLACES the body, and the manifest
does not survive that round trip — it would have to be parked somewhere first,
which means a store beside this cell and a correlation over the digest. What is
here today keeps the property that makes `access` worth having — **policy is
changed with an entry, not with code** — and the migration is a swap of phase A.

Tracked as [#435](https://github.com/mmeyerlein/meclaw/issues/435).

## `error_code`

| Code | When |
|---|---|
| `manifest_missing` | no ordered list to submit |
| `manifest_digest_mismatch` | the bytes are not the ones the digest was drawn over |
| `requester_unknown` | no `reply_to` on the envelope — nobody to attribute this to |
| `requester_not_permitted` | no policy row allows this requester |
| `scope_not_permitted` | a declaration addresses a scope this requester may not |

On phase B the colony's own code is passed through **verbatim** — no new string
is minted here.

## What it is not

- **Not a builder.** It composes nothing and can invent nothing: everything it
  emits was handed to it by somebody whose identity the substrate stamped.
- **Not a validator.** The colony validates. This cell refuses early where
  refusing early is cheaper than refusing at position k.
- **Not transactional.** What committed stays committed. The receipt says where
  it stopped so the rest can be sent again.
