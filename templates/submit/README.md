# `submit@2.3.0`

Two occupants behind one door, and the only reach onto the mutation door in the
whole tree. It asks who may submit — and, when the diff itself asks for it, whether
this manifest may author code and whether this identity may open its own push
lane. It answers none of the three. It does decide one thing, and only one: the
**form** of a subscribe edge, because that is the half no policy row can state.

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
place too. That is the whole architecture: the builder drafts and has
no such edge, so "the builder never applies" is a missing edge rather than a
promise, and a missing edge can only be missing **between two nodes**. That is
why drafting and submitting are two nodes and not one.

Since GH #556 the two are not siblings any more: the drafter is an occupant of the
OS level and this hive is an occupant of the **front door** beside it
(`/os/operator/submit`). The guardrail is unchanged, because it was never about
who stood where — it is about the edge that is absent between the drafter and the
submitter, and an absent edge is absent at every address.

## Phases, recognised positively

| Phase | Recognised by | What it does |
|---|---|---|
| A — a submission | the `manifest` slot carries the ordered **list** | digest and identity, then `[park, ask]` — the manifest into `store` under its digest, and the first question on `ask` |
| `in_verdict` | `hop.route` says so, re-stamped by the shell — **whatever the context carries** | which question it answers is read off the verdict's own `capability`, and off the id the ask minted for itself when the broker echoes only that. Never off a phase (GH #490). `allowed` → un-parks by digest, into the phase that answer books; anything else → deletes the parked row and refuses |
| `parked` | a store answer whose promoted `context.sub_phase` is `parked`, and no lane on the hop | the first question is booked: the remaining ones are asked in order, one per round, the row left where it is. A diff that authors code → `code.author`; a diff that draws an `in_pack` edge → the **form** check, then `affinity.subscribe`. Nothing left to ask → `[forget the park, remember the flight, submit]` |
| `authored` | a store answer whose `context.sub_phase` is `authored` | the same, minus the `code.author` question it just booked |
| `subscribing` | a store answer whose `context.sub_phase` is `subscribing` | `[forget the park, remember the flight, submit]`, unconditionally — every question over this manifest has been answered |
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

### Two round trips, two markers (GH #490)

This cell keeps two round trips it does not control — one to its store, one to
the broker — and until GH #490 both were recognised out of the same key space.
Context is persistent for the life of a chain and a cell emission inherits the
context it was handling, so a question asked from **inside** the un-parking
branch went out carrying `sub_origin=gate, sub_phase=parked`, the broker's edges
preserved the `sub_*` names by design, and the verdict came back wearing the
store's marker. The gate then read a **broker verdict** as a **store row**,
found no row in it, and answered `submission_check_failed` — with the row still
parked and nothing deleted.

It was structural rather than a missed case. The first question is asked while
the phase is `written`, which is not in the read set, so its verdict falls
through correctly; every later question is asked from a phase that **is** in the
read set, and there is no phase value it could carry that is not. So any
question after the first was unanswerable, and `subscribe: true` could only be
grown through `/colony/mutations`, which asks nobody.

Two rules hold it now, each sufficient alone, and they are the pair `access`
needed one level down (GH #481):

- **an interior marker never leaves the hive.** All three edges from `./gate` to
  the rim — `mutate`, `receipt` and `ask` — clear `sub_origin`, `sub_phase` and
  `sub_carry` with `delete_context`.
- **a lane beats a marker.** A delivery on `hop.route == 'in_verdict'` is a
  verdict whatever the context carries; a store answer has no `hop.route` at
  all, because `store` emits `operation` and `rows_affected` and the edge that
  brings it back adds nothing.

And the correlation itself stopped being a phase. The ask mints its **own id** —
`ask.<capability>.<nonce>` — and `access` answers a request with a `tool_result`
whose id is the id of the `tool_call` that asked, so the question a verdict
belongs to is read off the broker's `capability` first and off that id second. A
phase says what has been **booked** on the parked row; it never says which
question was put. The questions themselves are a table in the script — capability,
predicate, phase — so a fourth one is one row and nothing else.

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

## What the receipt says about the diff (GH #504)

Since 2.3.0 the receipt carries one more hop key, and it is a fact about the
**manifest** rather than about the door: `hop.registers_class` is `true` when
the submitted diff carried an `add_templates`, and `false` when it did not. It
is stamped on **every** receipt the renderer produces, refusals included, for the
reason § 2c of the development rules gives about lanes: an absent key and a
manifest that registered nothing must not look alike, or a subscriber cannot
tell a submitter that does not publish the fact from one that has none to
report. Whether the class is **live** is `error_code`'s absence, one key along —
two facts, two keys, and an edge that wants both reads both.

It has to be **remembered**, and that is the whole of the implementation. The
colony's answer arrives on a fresh trace carrying `outcome`, `applied`, `ids`
and — on a refusal — the position and the code. It says nothing about what the
declarations were, and the body the manifest travelled in is long gone by then.
So the flight row keeps one column of the diff it no longer holds (`registers`,
derived by the same `add_templates` read `authors_code` makes for the second
question), and the pop reads it back with the correlation.

The caller this exists for is the shell two levels up: `meclaw-os` draws
`./operator -> ./builder` on a committed `sub_receipt` that carries the key,
re-stamped `in_ingest`, because the librarian's corpus is a seed that stops
describing the library the moment a manifest registers a class (GH #496).
Until `meclaw-os@1.6.1` it read `./submit -> ./builder` on this hive's own
`receipt`; since GH #556 this hive lives inside the front door, whose
`sub_receipt` lane carries the receipt out for exactly the facts the shell
reads. The submitter is the
one cell in a tree that knows both facts — and it knows them without being told
anything about who is listening.

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

## The second question, derived from the diff

Since `2.1.0` this cell asks the broker **twice** when the manifest asks for it, and the
need is derived from what the manifest CARRIES rather than from what anybody says about it
(GH #446):

- any `add_nodes[].override_params` — or `swap_nodes[].with.params` — carrying a
  `script_inline` or `script_path` key, at any depth, because `override_params` is
  addressed per cell and a check that only read its root would pass every manifest that
  names the cell it is rewriting;
- or an `add_templates` at all, which registers a whole template class with an arbitrary
  script in it.

Either shape is the same act: putting executable behaviour into the colony that no
catalogue reviewed. So a second check-only question goes out with capability
`code.author`, over the same scope root, with the same `subject` — and the manifest stays
parked under the same digest while it is answered. Sequentially, and that is the design:
one row and one digest correlate both answers, and the verdict says which question it
belongs to in its own `capability` field.

A denial here is a **verdict class of its own** and carries a code of its own,
`code_author_denied`. "Who may submit" and "what may this submission bring with it" are
different refusals, and a caller that could not tell them apart would read a manifest it
is allowed to submit as one it is not. A manifest with no script asks no second question
and costs no extra round trip.

**That the verdict says which question it belongs to was only half true
until `access@2.4.1`** (GH #481). The broker recognised its own store round trip by
a context marker its interior edge promoted, and the marker outlived the round trip
— so the second question on the chain arrived looking like the first one's echo and
was answered out of the first one's carry, with the first one's capability and the
first one's `call_id`, without the policy table being read at all. `code.author` is
the only capability this cell ever asks second, so the whole path was unreachable in
a running colony no matter what the rows said. Nothing here changed: the fix is one
line of positive recognition per broker cell plus a `delete_context` on the hive's
exit edges. This hive carried the mirror image of the same defect in its own two
round trips and needed the same pair (GH #490, *Two round trips, two markers*).
These paragraphs record the correction because a reader would otherwise
have no way to tell a promise from a measurement.

**A `seed_rows` manifest asks no question of its own** (GH #456). Rows are not executable
behaviour, so the `code.author` derivation does not fire on them — and a capability beside
it would buy nothing: the reach of the write is the mutation scope, which is exactly what
the `colony.mutate` question is asked over, and a requester allowed to mutate that scope
could already `swap_nodes` the store outright. The eighth operation therefore travels the
gate exactly like a topology manifest: one question, one parking row, one digest.

## The third question, and the one thing this cell decides

Since `2.2.0` a manifest may also ask for an **identity door**, and that is a third
question with a shape of its own (GH #458).

A subscribe mutation is recognised by what it DRAWS: an `add_edges` entry whose
modifier re-stamps an arriving message onto the `in_pack` lane —
`{"modifier": {"set_hop": {"route": "'in_pack'"}}}`. A `set_hop` value is a CEL
expression, so the literal normally carries its own quotes; the bare word is
accepted too, because a derivation that saw only one of the two spellings would be
a check an author can step around by typing.

That lane is new in GH #458 and it is narrow on purpose: it is the only door
through which anything outside a sealed agent composite writes a durable
`system.*` slot into its brain. An edge that opens it is therefore exactly the act
that needs a capability of its own — not the messages that later travel it.

### The split: the gate checks the FORM, the broker checks the PERMISSION

**The gate checks the form.** For every `in_pack` edge in the manifest, both must
hold, and the check is made here:

- **the target is the requester's own hive, or a node the same declaration creates
  with `add_nodes`** — and every endpoint is read **resolved against the `scope` of
  the declaration it stands in** before anything is compared (GH #479). The
  resolution is the door's own arithmetic rather than a guess: the scope is written
  in the very declaration the endpoint is written in, and an *absolute* `add_edges`
  endpoint is refused at the door with `scope_out_of_bounds` — so a subscribe edge
  that can actually be drawn is always the relative spelling. Two branches, and the
  second one is narrower than the first:
  - *A brain draws its own door.* `to` equals the requester or the requester lies
    under it as a **path-segment** prefix. The requester is the brain and the edge
    ends at the sealed hive that brain lives in, so `/os/…/alex` does not lie under
    `/os/…/alexander` any more than `/oscar` lies under `/os`. A naive `startswith`
    here would hand a brain the identity door of a hive nobody named.
  - *A parent draws the door of the child it is growing.* The first branch can never
    hold there — when a level is grown the requester is the parent or the operator,
    and the brain whose door it is does not exist yet — so the edge may also end at a
    node **this same declaration** brings into the world, or **inside** one. There is
    no “somebody else” whose prompt is opened when the requester created the addressee
    in the same mutation. The *inside* half is
    [#561](https://github.com/mmeyerlein/meclaw/issues/561): the identity pack rides a
    v-lane now and the door ends at a brain RIM two storeys down
    (`<generation>/talky`), which came into the world in the same act as the
    generation — the same addressee spelled more precisely, never a second one. The
    comparison stays segment-wise, so a created `<…>/scribe` does not cover
    `<…>/scribe-of-somebody`. A creation in a *different* declaration does not count,
    and an address that merely exists does not count.

  Violation → `subscribe_target_not_self`.
- **the source is an affinity hive** — the last path segment of `from` is
  `affinity`. The hive PATH, and deliberately not `<…>/affinity/push`: `affinity`'s
  `params.ports` is empty, so the hive path is its only endpoint and an edge naming
  `./push` is refused at the door with `hive_port_boundary`. Demanding the port here
  would demand the one spelling that can never be applied. Violation →
  `subscribe_source_not_affinity`.

A failing form is refused **on the spot**, and the broker is never asked. A
malformed subscribe is not a permission question.

**The second branch is a correction, not a widening of the permission** (GH #479).
Until it landed, the target rule had one branch and read `to` as written, and the
two halves of the repository contradicted each other: `grow_level`'s `subscribe:
true` renders the only spelling the mutation door accepts, and that spelling was
the one this gate could not read — while the identity the rule asked for, the brain
itself, is not yet in the world at the moment its door is drawn. The documented
opt-in therefore had no reachable caller at all. What the branch adds is a *form*
permission over an addressee the requester created in the same mutation; the reach
of that creation is the mutation scope, which is exactly what the broker is asked
about, and `affinity.subscribe` is still asked.

**The broker checks the permission.** `affinity.subscribe`, asked over the same
scope root with the same `subject`, exactly like the other two. A denial is a
refusal class of its own, `subscribe_not_permitted`.

**Why the split is where it is.** A policy row cannot make this call. `policy`'s
`mismatch()` does four one-sided comparisons — `requester`, `capability`, `subject`
and per-key `scope_match` against the resource — and not one of them compares two
fields of the same request against each other. "The edge's target is the subject
itself" is not expressible in a rule at any shape, and `*` only asserts presence.
So a capability that could be granted for "any `in_pack` edge" would let one agent
open a door into **another** agent's prompt. The form has to be checked where the
manifest is readable, and the manifest is readable exactly here — `access` never
sees it, because a broker answer replaces the body it travelled in.

The order matters and is fixed: `code.author` first when both apply, then
`affinity.subscribe`. A manifest that is both survives **three sequential
questions over one parked row and one digest**. The store phase
(`parked` → `authored` → `subscribing`) records how far that row has got; which
question a verdict answers is the ask's own id and the broker's own echo, and
never the phase — see *Two round trips, two markers* above (GH #490).

Until GH #446 the prohibition lived in one line of the drafting prompt
(`templates/builder/brief`, "no bare cell type") and nothing in the tree enforced it: the
normaliser does not inspect the inner `add_nodes` keys, the fast lane passes
`override_params` through verbatim, this gate checked only the digest and the scope root,
and the door checks that a param key EXISTS, never what it contains. "Only what is in the
catalogue" was a sentence, not a boundary.

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
**edge** and never out of a body (R-AC-1), and the edge says `/os/operator/submit`
— this hive, asking. (It said `/os/submit` while this hive was an occupant of the
shell; since GH #556 it is an occupant of the front door, and the shipped policy
rows moved with the path. The template itself did not change: it is re-homed, not
rewritten.) The identity the substrate stamped on the submission travels as
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
| `code_author_denied` | the broker refused this manifest the authoring of code — no enabled rule grants `code.author` |
| `subscribe_target_not_self` | an `in_pack` edge whose resolved `to` is neither the requester's own hive nor a node the same declaration creates (nor anything inside one — #561). A **form** refusal: decided here, and the broker is never asked |
| `subscribe_source_not_affinity` | an `in_pack` edge whose `from` is not an affinity hive. Likewise a form refusal, likewise unasked |
| `subscribe_not_permitted` | the broker refused this identity its own push lane — no enabled rule grants `affinity.subscribe` |

On phase B the colony's own code is passed through **verbatim** — no new string
is minted here.

## What it is not

- **Not a builder.** It composes nothing and can invent nothing: everything it
  emits was handed to it by somebody whose identity the substrate stamped.
- **Not a validator.** The colony validates. This cell refuses early where
  refusing early is cheaper than refusing at position k.
- **Not transactional.** What committed stays committed. The receipt says where
  it stopped so the rest can be sent again.
