# `operator@1.2.0`

**One front door into the OS, and one place a submission lives.** A sealed hive at the
colony shell with one occupant per subject, reached by naming a lane and never a cell. It
decides nothing, and it remembers exactly one thing: a draft nobody has said yes to yet.

Since 1.1.0 ([#556](https://github.com/mmeyerlein/meclaw/issues/556)) the **submitter** is
one of the occupants. It used to be a hive of its own beside this one, which meant a
submission crossed two shell stations for what is one job and the road was only readable by
reading two templates. What moved is where the submitter *stands* — not what it may do: the
drafter and the submitter are still two nodes, they still share no edge, and the reach onto
`/colony/mutations` is still birth topology or nothing (ADR-0015 § Amendment 2026-08-31).

What it adds is one thing, and the whole template follows from it.

## Identity, never authentication

Read this section before wiring anything, because the distinction is the reason this hive
exists and the reason it is small.

The substrate stamps `envelope.reply_to` on a **cell's** emission. That is what the
submitter's gate reads:

```python
requester = str(env.get("reply_to") or "")
if not requester:
    receipt("requester_unknown", "no reply_to on the envelope -- nobody to attribute this to")
```

An agent inside the colony IS a cell, so it has a path and its submissions are attributed.
A person with a shell is not a cell: a `POST /messages` builds an envelope with no
`reply_to`, so the gate refuses every operator submission as anonymous — and the one route
that does work, `POST /colony/mutations`, walks past the gate and the broker entirely and
lands in the mutation log with no requester at all.

This hive lends that person a path. A request that arrives on `in_submit` becomes a message
emitted by `/os/operator/intake`, and that is the identity that reaches the door one edge
later. (Through 1.0.0 that cell was called `submit`; the name now belongs to the ref onto
the `submit` template beside it, because a ref inside a template is named after the template
it references — `docs/development-rules.md` § 8a, R1.)

**It is not authentication and it must never become authentication.** There is no token
here, no header check, no caller allow-list, no signature and no secret. This hive answers
*who the sender is*; it never asks *whether they are who they claim to be*. Authentication
belongs to a reverse proxy in front of the API — the default bind is loopback and that is
the whole of the story. Authorisation belongs to the capability broker behind this hive.
Neither is a lane here, and a pull request that adds one to this template is answering a
question this template does not ask.

## The lanes

| direction | lane | what it carries |
|---|---|---|
| in | `in_submit` | a manifest to apply: `manifest[]`, optionally pinned by `hop.manifest_sha256` — **or** the digest of a parked draft and no manifest at all. Two callers: a person at the rim, and an assistant inside the colony (`context.operator_caller == 'agent'`) |
| in | `in_dump` | `{target: <member path>, export_to?: <run dir>}` — produce a dump of that member's memory. `export_to` names the directory of ONE run, relative to the fence each holder's own store declares in `params.transfer.base_path`; without it every holder writes into its own name directly (GH #555, since 1.2.0) |
| in | `in_lifecycle` | `{op: 'birth' \| 'sleep' \| 'wake', scope, node?, edges?}` |
| in | `in_draft` | a manifest the baumeister drew for an operator. Parked, answered, **not applied** — the only sender is the level's own `./builder -> ./operator` edge |
| in | `in_verdict` | the broker's answer to the one capability question a submission asks — the only sender is the level's own `./access -> ./operator` edge |
| in | `export_done` | what the export lane answered |
| out | `export` | the export request on its way down, carrying `hop.target` and — when the caller named one — `hop.export_to` |
| out | `receipt` | what happened — **four senders, one lane** |
| out | `ask` | the capability question, raised by `./submit` and answered back in on `in_verdict` |
| out | `mutate` | the checked, attributed manifest, on its way to a door the colony's root draws or nobody does |
| out | `sub_receipt` | the submitter's own receipt, let out **only** when it carries `hop.error_code` or `hop.registers_class` |

`in_receipt` and `apply` were lanes of this rim through 1.0.0 and are edges inside it now:
`./intake -> ./submit` and `./submit -> ./intake`. Three lanes arrived in their place, and
none of the three is the front door's own — they are the submitter's, crossing this rim
because the broker and the mutation door are outside the hive it now lives in.

`params.ports` is `[]`. The hive path is the only address, so **a new subject is an occupant
directory and a door inside this hive** — `add_nodes` plus one edge — and never a change to
anybody's edges. That is the property the seal buys, and it is the same one `tools` buys.

`receipt` carrying four senders is deliberate: a caller subscribes to the LANE, never to
whichever occupant raised it. Nothing outside this hive has to learn that a lifecycle
refusal comes from a different cell than a submission's counts.

## An unknown lane is an answer

A request whose `hop.route` no occupant serves reaches the guarded default door and comes
back as an ordinary receipt with `hop.error_code = 'unknown_route'`, naming the lane
verbatim — including the empty string when the request carried none.

Not a dead letter. A dead letter is a message the colony lost; this is one it read,
understood as unaddressed, and answered in the same round. An operator who mistyped a lane
learns which one they typed instead of reading a queue.

## The occupants

| directory | reached on | what it does |
|---|---|---|
| `intake/` | `in_submit`, `in_draft`, the submitter's `in_receipt` | draws the digest, emits `apply` next door, parks a draft and renders the receipt |
| `submit/` | `in_apply`, `in_verdict` | a **ref** onto the `submit` template: the gate that asks the broker and the one node in the tree with a reach onto the mutation door |
| `export/` | `in_dump`, `export_done` | turns `{target, export_to?}` into a request on the lane the memory accepts, and renders `hop.export_hive` / `hop.seed_dir` out of what comes back |
| `lifecycle/` | `in_lifecycle` | composes a birth / sleep / wake manifest and hands it to `intake` next door |
| `unknown/` | nothing else fired | one receipt, `unknown_route` |
| `drafts/` | only `./intake` | a `store`: one row per parked draft, keyed by its digest |

`lifecycle` has no digest of its own and no edge onto `./submit`: it emits one declaration
internally to `intake`, which draws the digest over the bytes it forwards. One definition of
a manifest's identity per hive, in one cell.

### The digest

`intake` carries the same `canonical()` / `digest()` helper the builder and the submitter
carry, byte for byte, between the markers `# --8<-- digest-helper` and `# --8<-- end`;
`gh425_the_digest_is_one_definition` compares the copies.

A caller **may** pin the bytes with `hop.manifest_sha256`, and when it does the pin is
honoured here rather than three hops later. When it does not, the digest is drawn over what
arrived — and that is not a hole. The digest exists because a *draft* crosses a chat: shown
to a human, repeated by a model, and the bytes have to be checkable across that gap. An
operator posting a manifest directly IS the human, and the bytes are the ones they sent.

### Lifecycle, in the vocabulary the substrate has

There is no activate and no deactivate operation. Activity is fully **derived from the edge
table** (`crates/meclaw-colony/src/connectivity.rs`): a node is active iff it is connected
and its parent hive is active. So:

| word | operation | note |
|---|---|---|
| `birth` | `add_nodes` (+ `add_edges`) | `birth: "inactive"` is the one lifecycle state a declaration carries directly, and it is this hive's **default** — the reason to grow a cell in two steps is to look at it before it runs. Durable since [#491](https://github.com/mmeyerlein/meclaw/issues/491): a node born asleep is not woken by a recompute it was never the subject of |
| `wake` | `add_edges` | the edge IS the wake: the mutation that connects a node recomputes it active. It has to NAME the node — an edge drawn elsewhere reaches a sleeping node's recompute scope but never wakes it (#491) |
| `sleep` | `remove_edges` | cutting the last edge external to the unit derives the whole subtree inactive |

A `wake` or `sleep` that names no edge is **refused**, with a sentence saying why there is
nothing else it could mean. This hive does not guess which edge an operator meant, and it
does not invent an operation the substrate does not have.

## The halt before it runs (GH #474)

The baumeister's own contract calls what it emits a **proposal** — *"a sentence a human can read
before saying yes"*. When the rim learned to ask it for a draft (GH #469), that sentence started
travelling straight past the human: the only road out of the builder for an operator-initiated
round was this hive's `in_submit`, and a wish for a connector produced a correct three-edge
declaration **and a running `proxy` cell** in one round.

`in_draft` is the halt, and it is two acts on purpose:

```
POST /messages  {"target": "/os", "hop": {"route": "in_build"},
                 "body": {"messages": [{"origin": "user", "type": "text", "id": "",
                                        "text": "{\"request\": \"grow …\", \"scope\": \"/os/orgs/acme\"}"}]}}

   -> receipt   hop.draft_state    = "draft_ready"
                hop.manifest_sha256 = "<digest>"
                hop.draft_path      = "/os/operator/drafts"
                hop.declaration_count = 17
                body.manifest       = the declarations, verbatim
                "draft <digest> is ready and nothing has been applied: …"

POST /messages  {"target": "/os", "hop": {"route": "in_submit",
                                          "manifest_sha256": "<digest>"},
                 "body": {"messages": []}}

   -> the manifest is read back out of `drafts` under that digest and travels
      `apply` -> the submitter -> the gate -> the broker, exactly as any other.
```

The second act carries **no manifest**. That is the whole point: `submit <digest>` is a decision
about the bytes that were shown, not about the bytes the caller happens to be holding. A digest
nothing is parked under comes back as `digest_mismatch`, and nothing is submitted — the request was
never made rather than made and failed. The draft is **not deleted** when it is submitted: a
manifest the broker refuses is one its requester may submit again once the policy allows it.

`{"op": "submit", "digest": "…"}` in a `tool_call` turn is read as the same request, for the caller
that speaks the `lifecycle` occupant's dialect rather than the hop's.

**One act is still available, and it is a word in the wish.** `hop.auto_submit: true` on `in_build`
tells the shell to send the draft to `in_submit` the way it did before — for a rebuild script
replaying wishes somebody has already read. The default is the halt, because a proposal that
applies itself by default is the surprising one.

## What it is not

- **Not a decider.** Every manifest still travels the digest, the gate and the broker
  exactly as a model's manifest does. The sentence *"no cell in this hive has an edge onto
  the mutation door"* is **precised rather than dropped** with GH #556: the submitter lives
  here now, so `mutate` crosses this rim — but no cell of the *front door* raises it, none
  may, and the reach itself did not change hands. `/colony/mutations` is not an endpoint a
  mutation may draw at any scope, so the edge that finally carries the lane out of the
  colony's root is birth topology or it does not exist, and nothing that happens in here can
  acquire it.
- **Not the control loop.** The loop is an ACTOR with a charter of its own and the subject
  `/os/argus`; this is the hand of a person, subject `/os/operator`. The broker can rule
  the two differently, and that is the point of them being two.
- **Not the emergency exit.** `--apply` stays what it is: identity-less, deliberately, for
  the colony that cannot boot.
- **Not stateful — with one named exception, and it is a retraction.** This file said
  *"nothing is remembered between two requests"* through GH #469. Since GH #474 the `drafts` store
  remembers one thing: a manifest the baumeister drew and nobody has said yes to yet, under its own
  digest. It is a PROPOSAL rather than the state of a request — a colony that lost the store would
  lose offers nobody accepted, never a change it made. The round in flight is still remembered by
  the submitter, which owns its own store for exactly that.

## Wiring it: the contract at the level above

The shell (`meclaw-os`) is the only level that can wire this hive, because the broker, the
baumeister and the organisation container are its siblings only there.

```
.            --in_submit-->    ./operator        one lane per subject, at the hive path
.            --in_dump-->      ./operator
.            --in_lifecycle--> ./operator
./operator   --receipt-->      .                 the one lane out, guarded: NOT hop.submitter_kind 'agent'

./orgs       --build/apply-->  ./operator        set_hop route 'in_submit', set_context operator_caller 'agent'
./operator   --receipt-->      ./orgs            guarded: hop.submitter_kind == 'agent'
                                                 set_hop route 'in_build_result', build_op 'apply'

./builder    --manifest-->     ./operator        set_hop route 'in_draft', guarded:
                                                 context.build_caller == 'operator' AND NOT
                                                 context.build_auto_submit == 'yes'   (GH #474)
./builder    --manifest-->     ./operator        set_hop route 'in_submit', guarded:
                                                 context.build_auto_submit == 'yes'   (the one-act road)

./operator   --ask-->          ./access          set_hop route 'in_request', set_context
                                                 requester '/os/operator/submit', sub_ask '1',
                                                 sub_sha hop.manifest_sha256          (R-AC-1)
./access     --grant-->        ./operator        guarded: context.sub_ask == '1'
                                                 set_hop route 'in_verdict'
./operator   --mutate-->       .                 the one privileged lane, on out of the shell
./operator   --sub_receipt-->  ./builder         guarded: has(hop.error_code)
                                                 set_hop route 'in_receipt'            (GH #425)
./operator   --sub_receipt-->  ./builder         guarded: NOT error_code AND registers_class
                                                 set_hop route 'in_ingest'             (GH #504)

./operator   --export-->       ./orgs            set_hop route 'in_export', carries hop.target
                                                 and hop.export_to                     (GH #555)
./orgs       --export_done-->  ./operator
```

The four bottom edges arrived with the submitter (#556). Through `operator@1.0.0` they were
drawn from `./submit`, a sibling of this hive; the guards, the lanes and the re-stamps are
the same ones, one hive further out. The two that are **gone** from the shell entirely are
`./operator -> ./submit` on `apply` and `./submit -> ./operator` on `receipt`: they are now
`./intake -> ./submit` and `./submit -> ./intake`, inside this file, and they cross nothing.

**The round is told apart by the id.** `./submit` raises `receipt` for every submission it
handles, and `intake` marks the id it sends so that the marked id comes back: the submitter
hands it over verbatim off its own flight row. It has to be the id and not the context,
because the colony's answer to a mutation begins a **fresh trace** and nothing this hive
promoted on the way out survives the round trip.

The interior `./submit -> ./intake` edge is **unguarded**, and that is a consequence of
R-Zielfluss (a) rather than a looseness: `intake` is the only sender of `in_apply`, so every
receipt the submitter raises belongs to a round that started here. The marker still does its
work — it is what tells this hive's TWO callers apart on the way back out.

**Why the submitter's receipt also leaves on a lane of its own.** It fans out a second time,
to `./builder` at the level above: `in_receipt` when it carries a refusal (GH #425) and
`in_ingest` when a committed diff registered a template class (GH #504). That copy leaves on
`sub_receipt` and **not** on `receipt`, guarded here by exactly those two facts. The reason
is arithmetic rather than taste: `receipt` is the lane a caller subscribes to, and a
submission that answered on it twice — once rendered by `intake`, once raw from the gate —
is a submission whose caller cannot tell which answer is the outcome. An ordinary committed
receipt therefore has nothing to say outward and stays inside.

**The direct path is gone, and this is where it went.** `member → org → submit` used to
carry an assistant's `apply` straight to the submitter. It does not any more (R-Zielfluss
(a)): an assistant's `build` / `build_op == 'apply'` becomes `in_submit` at `/os/operator`,
and there is ONE submission front door rather than two. Nothing about the submitter
changed; what changed is who addresses it — and since #556 the submitter is not even a node
the shell could address: `/os/submit` no longer exists, and every road to the mutation door
runs through this hive.

**Two doors, one lane, and two carriers for which is which.** The `./orgs -> ./operator`
edge sets `context.operator_caller = 'agent'`; the `intake` occupant reads it and writes
the same fact into the correlation id as `op:agent:<id>` instead of `op:<id>`. On the way
back it reads BOTH, puts `hop.submitter_kind` on the receipt, and the shell routes on
THAT: an agent's receipt goes back down to `./orgs` as `in_build_result`, an operator's
leaves the colony on the rim. The marker never leaves this hive — the id on the receipt is
the one the caller used, because an assistant's fan-in waits for the id its own tool call
carried and a `tool_result` under a different one is a round that never ends.

**Why both carriers, and not one.** Neither survives every road, and the two failures are
disjoint:

| road | `context.operator_caller` | the `op:agent:` prefix |
|---|---|---|
| the gate refuses before parking (`requester_not_permitted`) | survives | **gone** — nothing was parked, so no flight row carried the id and it comes back empty |
| the manifest reaches the mutation door | **gone** — the colony's answer begins a fresh trace and carries no promoted context | survives — the flight row hands the marked id back verbatim |

One carrier alone is a lane that works until the outcome changes: read off the id only, a
refused agent round looks like an operator's and its receipt leaves the colony instead of
reaching the assistant that submitted. `crates/meclaw-cells/tests/operator_one_front_door.rs`
pins both roads.

**The identity is the front door's, for both callers.** `envelope.reply_to` on what leaves
`./operator/intake` is `/os/operator/intake` whether a person or an assistant asked, so an
agent's mutation is attributed to the front door and not to the assistant's own path. That
is the ruling as taken (R4, R-Zielfluss (a)): the broker rules the front door, and the
assistant reaches the door only through it. A per-caller subject is the `operator`-as-member
shape R4 parked for `>1 operator`, and it is not this template.

**Two names, two directions.** `in_dump` is what an operator asks this hive for; `export`
is what leaves it; `in_export` is what the memory accepts, and the shell's own
`./operator -> ./orgs` edge is what turns the second into the third. One string for a door
and an exit would have made the hive's default door catch its own answer.

**For whoever builds the export pass-through.** The shell re-stamps this hive's `export`
lane onto `in_export` — the standard shape this file already uses twice — and from there
`in_export` and `export_done` are the two names to relay **unchanged** at every level down
to the memory that answers: `{route: 'in_export', target: <member path>, export_to?: <run>}`
down, `export_done` up — one per holder since GH #555, carrying `hop.export_hive` and
`hop.seed_dir`. This hive resolves neither address and never sees a file: a path canonicalised
here would be a claim about a filesystem the operator does not stand on. The trigger is
`/os/operator/export` and nothing else, so an export is one thing an operator asks for
rather than a lane every level offers separately.

## Gates

- `crates/meclaw-cells/tests/operator_one_front_door.rs` — the seal, the dispatch, the
  unknown lane, the identity the submitter's gate accepts, and the lifecycle mapping.
- `crates/meclaw-cells/tests/gh446_model_authored_code_needs_a_capability.rs` — the other
  half of GH #446, in the submitter: a manifest that authors code is denied until
  `code.author` is enabled.
- `crates/meclaw-cells/tests/gh425_the_digest_is_one_definition.rs` — the digest helper,
  byte for byte, across every script that carries it.
- `crates/meclaw-cells/tests/gh474_a_draft_waits_for_a_yes.rs` — the halt: what `in_draft` parks
  and answers, what a quoted digest un-parks, what an unknown one refuses, and which of the two
  roads the shell takes with and without `hop.auto_submit`.
