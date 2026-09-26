# `llm-registry@2.3.0`

The one way to operate models in a colony -- as one hive of existing cell types. No new cell
type, no Rust, and **no model in any resolution**: a registry that needed a model to pick a
model could not repair one. Every push is decided by deterministic code over tables. The one
`llm` cell in here translates a cell's prose requirement into a catalogue row at most once per
change; its answer is a row like any other, its own model is a start value the registry never
resolves, and when it fails the last resolution stays.

Four cells:

| path | type | role |
|---|---|---|
| `store` | `store` | the catalogue: `models` (one model package per row), `tiers`, `overrides`, `subscribers`, `translations`, `open_questions`, `resolutions`, `incidents` |
| `select` | `code` | the read port -- requirements in, ONE resolution out, or a refusal |
| `hand` | `code` | the write hand -- resolves every subscriber by one precedence and pushes a model package into each brain whose package changed, **on call** |
| `translate` | `llm` | the translator -- a prose requirement and the active catalogue in, `{model_id, reason}` out, asked only when a (requirement, catalogue) pair has no answer yet |

No `probe`, no `clock`, and no control loop: the registry moves when it is told to, and the
translator is asked only by an op that finds a question unanswered. Incident-driven automatic
remapping is the target picture in [GH #130](https://github.com/mmeyerlein/meclaw/issues/130),
and a test in this repo pins the absence of the loop so it cannot arrive by accident.

Since 2.2.0 ([#855](https://github.com/mmeyerlein/meclaw/issues/855)) `meclaw-os` instantiates
this hive as `/os/llm-registry`, every brain an assistant is grown with becomes a subscriber of
it, and the package reaches the brain through the `in_model` door of `talky` and `cogny`. Since
2.3.0 ([#858](https://github.com/mmeyerlein/meclaw/issues/858)) so do the four llm cells of every
member's memory hive and the shell's own `argus/judge`, each through its composite's door.
`2.2.1` repairs the two doors: they clear the keys of the internal store round trip, so a
message from outside cannot pose as a store answer (see "Cells and lanes").

Since 2.3.0 ([#858](https://github.com/mmeyerlein/meclaw/issues/858)) a subscriber may state in
prose what it needs -- the `llm` param `requirement`, which its template writes -- and the
registry chooses a model for it from the catalogue (*Prose in, a model out -- once per change*,
below). The catalogue ships real rows of a hosted provider, and the operator keeps it on
`in_hand`.

**Upgrading to 2.3.0: the shell's judge and every member's memory now follow this hive.** In
`meclaw-os` `argus/judge` and the four llm cells of a member's memory hive run what the registry
resolves for their need, before the `.env` start value they fall back to. A push carries the
`base_url` of the catalogue row it resolved to, and an `llm` cell takes a run-time `base_url`
only when it is its own start endpoint or its origin is in `params.base_url_allow`; the shipped
cells have no list. Whoever points a cell at an endpoint of their own (`ARGUS_JUDGE_BASE_URL`,
`MEMORY_LLM_BASE_URL`) keeps it there with `pinned: 1`, with a `target` replacement onto a
catalogue row of that endpoint, or by not subscribing it -- `builder/compose`, born on
`LOCAL_LLM_BASE_URL`, is not announced for exactly this reason. Otherwise the cell refuses the
push (`invalid_input`, answered as an error on the cell's own exit), and a refused push is not
reported back to this hive today: `show` names the refused model until the next change. A lock
(`gh858_every_llm_cell_states_its_need`) holds every subscriber the shipped tree announces to an
endpoint the shipped catalogue's pushes can land on, in the default environment.

## Two layers: the start value and the registry

This is the division of labour, and everything else in this template follows from it:

| | the start value | this hive |
|---|---|---|
| answers | *what is this cell born on?* | *what does it run now, and why?* |
| comes from | the cell's params at instantiation (`${ctx.model}`), a `.env` key at boot (`${MODEL_*}`) | the catalogue, the replacements, the translation of its requirement, the tier index |
| states its need | `requirement`, prose in the template, immutable (since 0.46.0) | the translation of that prose against the catalogue, stored with its reason |
| changes take effect | at the next birth or boot | immediately, as a params-only message |
| holds | one string per cell | the whole model package, and the reason it was chosen |

So the start value is the **birth certificate** and the registry is the **living record**. They
do not compete: a cell is born on its start value, and from then on the registry is the only
thing that moves it without a restart -- and when nothing in the registry applies any more, it
puts the cell back on exactly that start value with a `$reset`. A cell cannot ask what it should
run, because **a cell cannot read the colony**; the registry pushes. Without `meclaw-os` there is
no registry, and a cell keeps its start value and the substrate's params road
(`docs/cell-types.md` § `llm`).

## The model package

A catalogue row is a **model package**: everything a brain needs to run that model and nothing
it is born with.

| package key | where the row keeps it |
|---|---|
| `model` | `model_id` |
| `base_url` | `base_url` (empty = the cell keeps its own endpoint) |
| `wire_dialect` | `wire_dialect` (`chat_completions` or `responses`; empty = the cell's own) |
| `model_prompt` | `prompt` -- the lines this model needs in every system prompt and no other model does; the brain puts them FIRST in its system part |
| `reasoning_effort`, `reasoning_wire`, `reasoning`, `thinking_budget`, `max_tokens`, `temperature`, `external_timeout_ms`, `provider_extra` | `package`, a json object; any other key in it is dropped and never reaches a cell |

The key list is the llm cell's own (`MODEL_PACKAGE_KEYS`, GH #853), and a template test holds
the hand's copy against it. What a package can **not** carry is the credential: `provider`,
`api_key` and the whole auth dimension stay immutable per cell, so a model behind a different
credential is a rebirth, not a message. A package that names a `base_url` is taken only by a
brain whose `base_url_allow` lists that origin (GH #853); a brain without the list refuses the
whole push, loudly, on its error lane. A row with an empty `base_url` is how a catalogue says
*this model lives where the cell already points*.

## The precedence

`hand` resolves each subscriber with ONE deterministic function over five tables -- `models`,
`tiers`, `overrides`, `subscribers`, `translations` -- and nothing else. Highest rank first:

| rank | what decides | reaches a pinned subscriber? | `reason` |
|---|---|---|---|
| `target` | an active `target` override whose `match` is a path prefix of the subscriber (on segment boundaries; the longest prefix wins, then the newest) | **yes** | `override_target` |
| `global` | an active `global` override whose `match` is the subscriber's BASE model -- its translation, else the tier's model, else its start value | no | `override_global` |
| `prose` | the translation of the subscriber's `requirement` (since 2.3.0); while there is none, the prose model the subscriber already holds | no | `prose_translated` |
| `tier` | the active row of the subscriber's `tier` -- for a subscriber that states no requirement, or one whose requirement has no active translation and holds no prose model | no | `tier_active` |
| `start` | none of the above: the start value the cell was born on | -- | `start_value` |

`prose` replaces `tier` where a requirement stands: a cell that said in prose what it needs is
resolved by what that prose chose, not by a tier. Until a new or changed requirement is
answered -- or while the translator fails, or its answer was refused -- the cell stays on what
it holds: the prose model it was on (`base_model` in `subscribers`, if still active), else its
tier, else its start value. So a change of prose moves a cell ONCE, when the answer is a row,
and never through its start value first. A subscriber without a requirement resolves exactly as
in 2.2.0.

A candidate whose model has no ACTIVE catalogue row is skipped and the next rank decides: a name
without a package would move a cell onto something nobody described. Only the start value needs
no row -- the cell already holds it. A `global` replacement replaces the base model and never
the result of another replacement, so `X -> Y` beside `Y -> Z` is two statements, not a chain.

**`pinned: 1` means "the start value, unless addressed".** A pinned subscriber resolves to its
start value or to a targeted replacement, and to nothing else: a tier or a global replacement
passes it over, and one journal line (`hand_skipped_pinned`) records that it did. A **targeted**
replacement reaches it anyway -- it is the operator's statement about exactly this cell, and a
pin is a statement about everything that is not addressed at it. The pin is part of the ONE
deterministic function above, so it does not remember what the cell ran before: pinning a
subscriber that a tier or a global replacement had moved resolves it again and pushes it back to
its start value (a `$reset`). To hold a cell on a particular model, pin it AND target it.

## The push

A push is a **params-only** message -- an empty `system` slot and no `messages` -- sent on `update` with
`hop.subscriber` (the brain's cell path) and `hop.rank`:

```json
{"system": {},
 "params": {"model": "provider-b/model-large", "base_url": "https://gateway.example/api/v1",
            "wire_dialect": "responses", "reasoning_effort": "medium", "max_tokens": 4096,
            "model_prompt": "…",
            "$reset": ["reasoning_wire", "reasoning", "thinking_budget", "temperature",
                       "external_timeout_ms", "provider_extra"]}}
```

It carries the WHOLE package and `$reset` for every package key the package does not set, so no
key of an earlier package survives a model change. Falling back to the start value is the reset
alone: `{"system": {}, "params": {"$reset": [<every package key>]}}`. The `llm` cell merges the overlay into
its live params, persists it in its own `cell.db` and emits **nothing** -- no inference, no
provider call, no tokens -- and it survives wake and respawn, because the overlay is replayed
over the birth params.

**Only when the package changed.** Each subscriber row keeps `package_hash`, the hash of what
was last pushed (empty for *nothing pushed, the start value*). An op that resolves a subscriber
to the same bytes sends nothing, so repeating a replacement, re-announcing a known brain or
remapping a tier onto the model it carries already costs no message at all. The empty `system`
slot is what makes the body a valid message at all (`ubf-body.json` wants `system`,
`messages` or `attachments`); a body carrying only `params` would be dead-lettered on the way.

## The view per cell: `show`

`{"op": "show"}` answers on `answer` with one entry per subscriber -- `cell_path`, `model_id`,
`rank`, `reason`, `since`, `pinned`, `tier`, `start_model`, and since 2.3.0 `requirement` and
`because` (the translator's one sentence, when the rank is `prose`; while a changed requirement
holds the prose model of the old one, `held until the new requirement is answered` instead of
the old sentence) -- plus the active overrides
and the precedence literal. `{"op": "show", "cell_path": "<prefix>"}` narrows it to one cell or one
generation. The view is the subscriber rows themselves: every op that resolves a subscriber
writes what it resolved to, so the answer is one read and never a second resolution that could
disagree with the push. It is ONE page of `subscriber_rows` (see *Settings*), and it says so:
`truncated: true` when the bound cut it -- narrow it with `cell_path`, or raise the bound.
`show` reads and changes nothing, so it is the one op that needs no actor.

**`show` says what was sent, not what runs.** A brain answers a push with nothing, so the
registry never learns what it runs. A push the brain refuses -- a package whose `base_url` is
outside the brain's `base_url_allow`, an `external_timeout_ms` its backstop does not clear --
leaves as an error on the brain's own lane (in a `talky`, its `./errors` collector), and the
brain keeps its previous params, while `show` still names the package it was sent. The answer
carries that sentence in its `view` field, so no reader of the JSON has to know it from here.

## The ops on `in_hand`

Every command is a `tool_call` turn carrying a JSON object, and the edge that brings it in MUST
promote the deciding identity to `context.actor` -- a command with no actor is refused
(`no_actor`), `show` excepted. Each one is acknowledged on `ack` (`accepted` or `rejected` plus a
`reason_code`) after its pushes, and journalled in `resolutions`. An op that may concern every
subscriber (`remap`, `override_set`, `override_clear`, `model_upsert`, `model_retire`,
`retranslate`) reads them page by page and acknowledges once, after the last page, with the
totals and the number of `pages`. An op that may ask the translator says how many questions it
asked (`translations_asked`); the answers arrive after the ack.

| op | body | does |
|---|---|---|
| `override_set` | `{"scope": "global", "match": "<model X>", "model_id": "<model Y>"}` | "model X everywhere -> Y": every unpinned subscriber whose base model is X moves to Y |
| `override_set` | `{"scope": "target", "match": "<path prefix>", "model_id": "<model Y>"}` | "this brain (or everything under this path) -> Y", pinned or not |
| `override_clear` | `{"id": "<override id>"}` | the replacement goes inactive; every subscriber it reached is resolved again |
| `reset` | `{"cell_path": "<subscriber>"}` | clears the targeted replacements whose `match` is exactly this path and resolves it again. A prefix replacement that also covers siblings stays |
| `remap` | `{"tier": "<tier>", "model_id": "<model>"}` | the tier row supersedes; its subscribers are resolved again |
| `subscribe` | `{"cell_path": "<brain>", "start_model": "<start value>", "requirement"?: "<prose>", "tier"?: "<tier>", "pinned"?: 0 \| 1}` | makes a brain a subscriber, or corrects its start value, requirement, tier or pin. With a requirement the start value may be empty |
| `model_upsert` | `{"model": {"model_id": "<id>", <any catalogue column>…}}` | writes one catalogue row, merged into the stored one; `status` defaults to `active`. Every subscriber is resolved again |
| `model_retire` | `{"model_id": "<id>"}` | the row goes `retired` and is never a result again; every subscriber is resolved again |
| `retranslate` | `{}` | asks the translator every question the current catalogue has not answered -- the retry after a failed round |
| `show` | `{"cell_path"?: "<prefix>"}` | the view per cell, on `answer` |

Two replacements in the neutral form a colony writes them -- one model moved everywhere, and the
conversation brain of one assistant moved on its own:

```json
{"op": "override_set", "scope": "global", "match": "provider-a/model-mid", "model_id": "provider-b/model-large"}
{"op": "override_set", "scope": "target",
 "match": "/os/orgs/acme/members/alex/assistants/scribe/talky/brain", "model_id": "provider-b/model-large"}
```

Setting a replacement for the same `(scope, match)` again supersedes the old row rather than
overwriting it; nothing in `overrides` is ever deleted. The refusals are named: an unknown or
retired model (`unknown_model`), a targeted `match` that covers no subscriber
(`no_subscriber`), an empty `match` or an unknown `scope` (`incomplete_command`), a clear of an id
nobody set (`unknown_override`), a reset of a path that is no subscriber (`no_subscriber`), a
catalogue column that does not exist (`unknown_model_field`) or has the wrong type or size
(`invalid_model_field`: a `prompt` over 8 KiB -- what every llm cell refuses as
`model_prompt` --, `strengths` over 2 KiB, a `model_id` that is not one token of
`[A-Za-z0-9._:/@+-]` of at most 128 characters; refused, never cut), a retirement of a model that is not active (`unknown_model`), and a
requirement over 2 KiB (`requirement_too_long`).

## Prose in, a model out -- once per change

A requirement is a few sentences of English in the cell's template -- the role, the latency it
can afford, how much context it reads, how deep it has to think, what it may cost -- and never a
model name (`docs/cell-types.md` § `llm`, param `requirement`). The registry turns it into a
model like this:

1. **The key.** A translation is keyed by the pair `(requirement_hash, catalogue_hash)`: the
   first 16 hex digits of the sha256 of the requirement (the cell's stderr line shows the first 8
   of the same digest) and of what the translator is shown of the catalogue -- every ACTIVE row's
   id, provider, context window, prices, `caps`, `traits` and `strengths`. A price change or a
   retired model is a new question; a new package, prompt block, endpoint or note is not.
2. **Asked once.** `subscribe` (and an announcement), `model_upsert`, `model_retire` and
   `retranslate` ask `./translate` exactly for the pairs that have no row -- once per distinct
   requirement, not once per subscriber, and never while a push is delivered. `remap`, the
   replacements and `reset` only read. Once also while a question is OPEN: the op that asks
   writes it into `open_questions` in the same bundle as its other writes, and reads, in that
   bundle and before its own row, whether somebody else holds it -- the store runs one bundle
   after the other, so of two ops asking the same pair only the first one asks; the second one
   finds the question open and asks nothing (its `translations_asked` still counts the claim).
   The answer settles every subscriber of the requirement, whoever asked. An open question
   closes with its answer, its refusal or its failure, and expires after 120 s -- past the
   translator's 90 s backstop, so a question alone in the translator's mailbox is answered or
   failed by then, and past that the question may be asked again. The backstop bounds one call,
   not the wait in the mailbox: of several questions queued behind each other a late one can
   outlive its claim, and a trigger in that window asks it twice -- one more call, the same
   result (the second answer finds the first one's row, `translation_duplicate`). A question
   whose catalogue moved between an op's read and its write is asked against the catalogue as
   it stands, and its claim moves with it: the op that moved the catalogue may never have seen
   the subscriber. Nobody checks the moved pair for a question already open or answered, so
   should the mover have asked the same pair for another subscriber of the requirement, it is
   asked twice -- one more call, the same result (the second answer finds the first one's row,
   `translation_duplicate`).
3. **Checked, then stored.** The answer is one JSON object, `{"model_id", "reason"}`. It is a
   candidate: a model the catalogue does not carry as active is refused
   (`translation_outside_catalogue`, the id cut to one line of 128 characters in the journal),
   an answer that is not that object is refused
   (`translation_unreadable`), a failed call is journalled (`translation_failed`), and in all
   three cases nothing is stored and nothing moves. An accepted answer becomes one row in
   `translations` with its reason (on one line: control characters and line breaks become
   spaces, at most 400 characters), and the subscribers of that requirement are resolved again.
4. **Deterministic after that.** Every resolution reads the row for the current catalogue -- of
   two rows for one pair the first stored -- or, while the current catalogue has no answer yet
   or its answer was refused, the newest row of any catalogue whose model is still active. A
   retired model is never a result: without an active translation the cell keeps the prose
   model it holds, if active, else falls to its tier, else to its start value.

`show` names the rank `prose` with the translator's sentence as `because`. The translator itself
is a plain `llm` cell with a **start value** (`${LLM_REGISTRY_TRANSLATOR_MODEL:-…}`,
`${LLM_REGISTRY_TRANSLATOR_BASE_URL:-…}`, the key `${OPENROUTER_API_KEY:-}`); no `update` edge
reaches it and the registry never resolves it, so a broken translator can never stand between the
registry and a repair. Without a key the registry still boots, resolves and pushes, and every
translation fails into the journal. The instructions it is given travel in the question's
`system` slot, and it keeps no history: each question carries the whole catalogue.

## How a brain becomes a subscriber

**From the tree, in `meclaw-os`.** The builder's `grow_level assistant` draws, beside the level,
a declaration at `/os/orgs`: one push edge per brain (`talky`, `talky-chat`, `cogny`) onto the
composite's `in_model` door, and one edge that turns every mutation receipt reaching the
generation into an **announcement** -- a message with the brains on
`context.model_announced`, `[{"cell_path": …, "start_model": …, "requirement"?: …}]`, and the generation itself
on `context.model_generation` -- which `meclaw-os` hands to this hive's `in_hand`, stamping
`context.model_announcer`. The hand makes a subscriber of each brain it does not know, pushes at
once if a replacement already covers its start value, and answers nobody: nobody asked, so
nobody waits. A known brain announced again is nothing -- the hand reads each announced path
exactly and writes a new row as delete-then-insert, so two announcements in flight together
still leave one row -- and a lost row comes back with the next receipt or the boot's. That
replaces what used to be this template's most likely fault -- a subscriber list kept by hand,
drifting away from the tree. Since 2.3.0 an entry may carry the brain's `requirement`, as its
template states it: a new or changed one is asked once, an entry without the key leaves the
stored one as it is, and with a requirement an empty start value is taken too (a cell born on an
empty model key is still served by its translation). An empty start value in an announcement
says nothing about the start value, like a missing requirement: it makes a new row with none,
and it never clears one the row holds -- `meclaw-os` announces its own judge that way, since the
shell substitutes nothing, and a start value an operator stated with `subscribe` survives every
later receipt. A requirement over 2 KiB leaves its entry out, journalled
`hand_requirement_too_long`.

**An announcement is never a command, and it speaks for one generation.** Whatever arrives with
`context.model_announcer` is read as an announcement and nothing else: a `tool_call` turn riding
on it is ignored, a chain that still carries an `actor` from upstream is no operator here, and
without `context.model_announced` it is refused (`announcer_carries_no_announcement`). The lane crosses
from the organisations into the shell, and a turn on it could have been written by a model. Only
brains INSIDE `context.model_generation` are taken; a brain it names outside is journalled
(`hand_announce_outside_generation`) and neither subscribed nor rewritten, and an announcement
with no generation is refused (`no_generation`). The generation and the brains are both
context, which only an edge writes, and the submit gate lets exactly the edge the builder
renders through -- so the start value of a brain is corrected only by the road of the generation
it was born in. A list on `hop.subscribe` is never read (it only keeps the message on this lane):
a hop key survives a transit through a hive, so a foreign hive could rewrite it on a genuine
announcement's way back, and until 2.2.0's last fix it was where the brains rode. These
refusals are journalled, never acknowledged.

**What a push can and cannot be made to do.** A push is a params-only message addressed by
`hop.subscriber`, and the brain that receives it does not compare that address with its own
path. What keeps a push in its brain is the road: the push edge carries only the pushes
addressed to one cell directly in its composite, and the submit gate refuses any edge that writes
`hop.subscriber` or `hop.subscribe`, removes `hop.route`, or stamps a route it cannot list
(`templates/submit/README.md` § The model registry's road is a form too). An edge that leaves
the road's keys and its route as they are -- no modifier, one that writes other keys, or
`set_hop route "hop.route"` -- drawn at the container onto somebody else's composite, or onward
from an addressed one, is not stopped there: it copies pushes into a brain nobody addressed,
and under the shipped broker default it is permitted at `/os/orgs` exactly as a `swap_nodes` on
that brain is. Binding the push at the receiver is an `llm`-cell change; until it exists a
policy that narrows `colony.mutate` is the place this is held. With `code.author` granted (off
by default) the road is not held at the gate at all: an `add_templates` registers a class whose
inner edges may write `model_generation`, `model_announced` or `set_hop subscriber`, and the
gate reads `add_edges` only.

**By command, anywhere.** `subscribe` on `in_hand` writes the same row. It is the upgrade step for
a brain grown before 2.2.0, together with its push edge (an announcement edge is drawn only in the
manifest that grows its generation), and the way in for a tree without `meclaw-os`, where each
subscriber also needs its own `update` edge (`hop.route == 'update' &&
hop.subscriber == '<path>'`), restamped onto the lane its receiver takes.

## Cells and lanes

```
llm-registry/                  hive  -- scope marker, internal edges
  store/                       store -- the catalogue (7 tables)
    seed/{models,tiers}.jsonl
  select/                      code  -- read port: tiers -> models -> resolve -> journal
  hand/                        code  -- write hand: one bundled read -> resolve -> writes + pushes
  translate/                   llm   -- translator: requirement + catalogue -> {model_id, reason}
```

A `code` cell has no `cell.db`, so a lane that needs several reads keeps its state on the wire:
`select` and `hand` emit their phase and their carry on the **hop**, the internal edge promotes
both to **context**, and the store's answer brings them back. The store round trip *is* the
cell's memory. `hand` reads everything an op resolves against in ONE store bundle (four
`tool_call` turns and the op's own probes, one reply, GH #295) and writes everything it decided
in one more. A page whose subscribers state requirements is read a second time, with three reads
per requirement in the same bundle (the translation for this catalogue, the newest one with an
active model, and whether the question is open), before anything is written. An op that asks
writes its claims into its write bundle, and the store's echo of that bundle (phase `claimed`)
is where the question leaves. The
translator's round trip is kept the same way (`./hand -> ./translate` promotes phase and carry,
`./translate -> ./hand` brings them back), and the door rule below covers it as well: nothing
from outside can pose as a translation either.

Because those context keys ARE the lane's memory, nothing from outside may carry them in. Since
2.2.1 the two doors (`. -> ./select` on `in_select`, `. -> ./hand` on `in_hand`) delete
`lr_phase`, `lr_carry` and `registry_origin` on the way in. Until then a sender at the rim
could set `hop.operation` and, on the edge it draws, `context.lr_phase` -- and the hand ran a
command from `context.lr_carry` against a world the sender wrote itself, with no `context.actor`
behind it, while `select` answered and journalled a resolution of the sender's own. Now such a
message reaches the cell as an echo with no phase and ends there.

`params.ports` is empty (GH #228): every endpoint below is the registry's own path, and what a
caller wants rides on `hop.route`.

| lane | direction | what travels |
|---|---|---|
| `in_select` | in | the request as a `tool_call` turn (`{tier?, capability?, max_cost?, min_context?}`); the edge promotes the asker to `context.asker` |
| `answer` | out | a lookup's result (`hop.resolved` / `hop.reason_code` / `hop.model_id` / `hop.tier`), or the view `show` returns |
| `in_hand` | in | a command (the ops above); the edge **MUST** promote the deciding identity to `context.actor`. Or an announcement from the tree, over an edge that stamps `context.model_announcer` and `context.model_generation` instead |
| `ack` | out | `accepted` or `rejected` plus a `reason_code`, and the counts: `pushed`, `unchanged`, `skipped_pinned` (and `subscribers`, `pages` for the ops that page) |
| `update` | out | one model package for one subscriber -- `hop.subscriber` names which, `hop.rank` says why |
| `error` | out | the lane could not be served. The parent MUST wire it |

`incidents` has no writer among these lanes, and `models` had none until 2.3.0 brought
`model_upsert` and `model_retire`: both can be maintained over a **parent's boot-graph edge
straight into `./store`**. The hive port seal (`params.ports: []`) refuses a
runtime mutation that names an interior path (`hive_port_boundary`), but the bootstrap is
deliberately outside that check, because whoever writes a parent's birth topology has the whole
tree in front of them (`crates/meclaw-colony/src/mutation/port_boundary.rs`, ruling of
2026-08-15). That operator edge is how the tests in this repo file an incident and add a
catalogue row.

### Identity comes from the edge, never from the body

`select` takes its asker from `context.asker` and writes it into the journal line. `hand` takes
its actor from `context.actor` -- and **refuses the command without one** (`no_actor`); the actor
becomes the `decided_by` of every tier row and override it writes. Every op but `show` reaches out
and changes what other cells run, so the identity behind it has to be something an edge wrote;
`show` only reads, and runs without one. A
cell knows no sender; a body is written by whatever produced the message, up to and including a
model. An edge is written by the colony.

## The four limits, plainly

None of these is a bug, and none of them is fixable inside this template.

1. **The registry has to push, so consistency is eventual.** A cell cannot resolve anything
   itself -- cells cannot read the colony. A change reaches its subscribers one message at a
   time, and a call already in flight finishes on the old model. The registry knows what it
   **sent**; it never knows what a cell **runs** -- a push a brain refused (a `base_url` outside
   its `base_url_allow`) is an error on the brain's lane, not a row here.
2. **A subscriber needs a road.** A row without an edge is a push that dead-letters. In
   `meclaw-os` the builder draws both halves with the generation; anywhere else, and for a
   brain grown before 2.2.0, somebody draws them, and a push for a path with no edge
   dead-letters where the road ends.
3. **The package is the boundary, and the credential is outside it.** Everything in the package
   is run-time mutable; `provider`, `api_key` and the auth dimension (`auth`, `auth_ref`,
   `oauth_*`) are not, and a `base_url` moves only inside the cell's own `base_url_allow`. A
   model behind another credential is a rebirth, not a message. In practice the wall rarely comes
   up: as long as everything hosted goes through one gateway and everything local through one
   OpenAI-compatible endpoint, the credential is a constant and a package is a string swap.
4. **Costs are cent integers, not prices.** A store column is `text`, `int` or `json` -- there is
   no float. `cost_in` and `cost_out` are **cents per million tokens**, kept that way on purpose:
   a price stored as text would sort lexicographically, which is the wrong answer wearing the
   shape of an order. Actual spend is not here either; token counts live in the `meta` slot of an
   `llm` emission and in the message log.

There is a fifth thing worth saying: **`incidents` is a journal and nothing else.** A row in it
does not move a tier, does not touch `models`, and does not cause a single message to leave the
hive. Acting on it is a decision somebody makes, and it leaves as a command over `in_hand`. A test
pins this: an incident row changes no other table and emits nothing.

## The tables

| table | what it is | who writes it |
|---|---|---|
| `models` | the catalogue: id, provider, base_url, wire dialect, context window, `cost_in`/`cost_out` in cents per million, `caps`, curated `traits`, status, note -- since 2.2.0 `package` and `prompt`, since 2.3.0 `strengths` (prose the translator reads) | `seed/models.jsonl` at instantiation, then `hand` (`model_upsert`, `model_retire`) or the **boot-graph edge** |
| `tiers` | the index: `tier -> model_id`, with `since`, `decided_by`, `active` | `seed/tiers.jsonl` at instantiation, then `hand` (`remap`) |
| `overrides` | the replacements: `id`, `scope` (`global` \| `target`), `match`, `model_id`, `since`, `decided_by`, `active`. Since 2.2.0 | `hand` (`override_set`, `override_clear`, `reset`) |
| `subscribers` | which cell is served, its `tier`, `pinned`, `start_model`, since 2.3.0 its `requirement` and `requirement_hash` -- and what it resolved to: `model_id`, `rank`, `reason`, `since`, `package_hash`, and the prose base it holds (`base_model`, `base_rank`, `base_source`, `because`) | `hand` (`subscribe`, the announcement, every resolution), or the boot-graph edge |
| `translations` | since 2.3.0: one row per answered `(requirement_hash, catalogue_hash)` -- `model_id`, `reason`, `at` | `hand`, after checking the translator's answer |
| `open_questions` | since 2.3.0: the questions asked and not yet answered -- `requirement_hash`, `catalogue_hash`, `claim`, `at`; a row counts as open until its answer, refusal or failure, 120 s at most; an expired row stays until the next claim of its pair removes it | `hand` |
| `resolutions` | the journal: every lookup and every push, granted, refused or skipped -- with `rank` and `source_id` (the override id, `tier:<name>` or `translation:<requirement_hash>`) since 2.2.0; since 2.3.0 also every translation stored, refused or failed | `select`, `hand` |
| `incidents` | the field log: `model_id`, `kind` (`rate_limit`, `outage`, `slow`), `at`, `detail` | **boot-graph edge only** |

The new columns and the `overrides`, `translations` and `open_questions` tables are added to an existing `cell.db`
at the next spawn (`ALTER TABLE ADD COLUMN`, strictly additive), so a registry that ran 2.1.0 or
2.2.0 keeps its rows -- and keeps its old catalogue too, because a seed applies only to a new
store: bring the shipped rows in with `model_upsert`.

The store bounds the one write the **substrate** would answer on its behalf:
`contract.write_surface: "internal"` (GH #260) refuses a `transfer` `import` whose sender sits
outside `/…/llm-registry`, so nobody can pour a catalogue, a subscriber list or a translation in
past the hand.
The other half, `store`'s own `params.write_surface` (GH #132), stays **open on purpose**: it
would bound `handle()` to senders inside the hive, and `incidents` (and `models`, over the boot
edge) are written by an operator outside it by construction. A test pins both the declaration and the reason.

Nothing here is ever deleted: a tier change and a replacement set again both set the old row
`active: 0` and insert a new one, and a cleared replacement goes inactive, so what a tier or a
replacement used to mean stays readable. As everywhere in this substrate, that is a promise of
the **template** -- the store has a `delete` op and would happily run it.

## The seed

`store/seed/models.jsonl` ships **six rows a hosted provider lists publicly, as of
2026-09-26** -- id, context window and list price in cents per million tokens, all behind one
OpenAI-compatible gateway endpoint (`https://openrouter.ai/api/v1`, `chat_completions`), each
with a sentence of `strengths` for the translator -- plus one row retired on purpose:

| model | context | in / out (cents per million) | status |
|---|---|---|---|
| `anthropic/claude-opus-5.5` | 1 000 000 | 400 / 2000 | active |
| `anthropic/claude-sonnet-5` | 1 000 000 | 200 / 1000 | active |
| `google/gemini-3.8-flash` | 1 048 576 | 75 / 375 | active |
| `openai/gpt-6-luna` | 1 050 000 | 10 / 50 | active |
| `openai/gpt-6-sol` | 1 050 000 | 200 / 1000 | active |
| `qwen/qwen3.8-flash` | 1 000 000 | 15 / 47 | active |
| `openai/gpt-5.6-luna` | 1 050 000 | 20 / 120 | retired |

`tiers.jsonl` indexes them as `light` (`openai/gpt-6-luna`), `mid` (`anthropic/claude-sonnet-5`)
and `strong` (`anthropic/claude-opus-5.5`). Prices and listings move; every row says in its
`note` when it was read, and the operator keeps the catalogue with `model_upsert` and
`model_retire`. No row carries a package or a prompt block: how a deployment runs a model
(reasoning effort, token budget, the lines a model needs) is its own decision, and the form is in
*The model package* above. The retired row is retired in this catalogue, not by the provider:
the row that replaced it does the same job for less, and a retired row is never a result.
`traits` stays empty -- a curated score is a judgement a deployment makes about its own work.
`subscribers` is deliberately **not** seeded -- an invented subscriber path would push params at
a cell that does not exist -- and neither are `overrides`, `translations`, `open_questions` and `incidents`, which
start empty by definition.

Seed applies only on `OpenStatus::Created`; a re-open never overwrites it.

## Settings

The page bounds are params of the cells that read them, not environment
([#138](https://github.com/mmeyerlein/meclaw/issues/138), ruling R-0904-6). Each exists as a
`params` key of its cell, as a `contract.settings` entry beside it, and as the literal the shipped
script falls back to. What is left in `.env` for this template is the translator's start value
and nothing else: `LLM_REGISTRY_TRANSLATOR_MODEL` (default `anthropic/claude-opus-5.5`),
`LLM_REGISTRY_TRANSLATOR_BASE_URL` (default the gateway above) and `OPENROUTER_API_KEY`, all
three with defaults, so the registry boots without any of them.

| param | cell | default | what it bounds |
|---|---|---|---|
| `tier_rows` | `./select` | 200 | the tier index read of one lookup |
| `model_rows` | `./select` | 500 | the active-model read of one lookup |
| `subscriber_rows` | `./hand` | 200 | the subscribers one hop reads, resolves and may push to. `remap`, `override_set` and `override_clear` page on until a read comes back short, and `subscribe`, `reset` and an announcement read their own paths exactly, so it bounds the work of a hop, never who is reached; `show` answers one page and says `truncated` |
| `catalog_rows` | `./hand` | 500 | the active models, tiers and overrides one op resolves against |

```json
{"add_nodes": [{"name": "llm-registry", "template": "llm-registry",
  "override_params": {"select": {"model_rows": 2000}}}]}
```

## Tier names are data

`light`, `mid` and `strong` are what the seed carries. They are rows, not code: nothing
in `select` or `hand` knows a tier name, so adding `embed` or `think` is an insert and not an
edit. Pick names for the **role** a tier plays, not for a model class -- *strong* will still mean
something in two years when a particular model class does not.

## What is deliberately not here

- **No automation.** No probe, no health check, no latency budget, no auto-remap. The hand moves
  when it is told to. See GH #130 for where this goes.
- **No natural language on the read port.** `select` parses a JSON object, not a sentence. Prose
  enters in one place, a subscriber's `requirement`, and it is translated once per change -- a
  question a cell could ask at run time would put a model on the resolution path.
- **No regulating loop around the translator.** It answers when an op finds a question
  unanswered, not on an incident, a timer or a price feed (GH #130).
- **No naming scheme for start values.** The keys a cell is born on (`${ctx.model}`,
  `${MODEL_*}`, a judge's own key) stay as they are; the view per cell is `show`, and a model
  changes here rather than in an environment.
- **No embed authority.** The memory hive keeps its own `emb_models` table and stays the
  authority for what it actually runs -- it has a measured degradation behaviour and a
  binarisation generation hanging off that `model_id`. The catalogue may list embedding models;
  it does not steer them.
- **No export lane.** The substrate does have a counterpart to the seed -- the `transfer` body
  slot, `export` and `import`, answered before `handle()` for every cell with a `cell.db`. This
  template declares no lane for it and bounds the `import` half to the hive scope
  (`contract.write_surface`), so a backup from outside is a read: an `export`, or a `select` plus
  a file.
