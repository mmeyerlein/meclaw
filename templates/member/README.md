# `member@1.4.0`

One person, as a level. **Three holders, three open containers and one cell of
its own** — seven nodes and fifty-one edges.

| holder | what it holds |
|---|---|
| [`affinity`](../affinity/README.md) | **identity and meaning** — the curated record of who this person is and who their people are to them. Curated, fail-closed, quotable: it answers *who is X to me* and it is the only thing that answers it. |
| [`memory-hive`](../memory-hive/README.md) | **observations**, tagged with the participant set they were learned in. Raw, allowed to be wrong, carrying a confidence — this is what was said, not what it means. |
| [`firewall`](../firewall/README.md) | **the screen**. Every inbound turn is measured before it reaches anything of this person's, and the verdict is a comparison or a clock, never a model. |

Beside them stand `assistants` and — since 1.3.0 — `channels` and `apps`, three
real, empty, open containers, and `export-sink`, the one `code` cell this level
owns.

Memory produces, affinity decides, the screen measures. Three holders, three
jobs, no overlap (GH #122, the three-holders ruling of 2026-08-19). Since
[#471](https://github.com/mmeyerlein/meclaw/issues/471) all three of them also
travel: one word at this level's door walks all three out as documents, and a
member grown from them is born with the memory, the record AND the screen.

**The fourth thing every agent of this person shares is the CHANNEL they are
reached on, and since 1.3.0 it is here too**
([#454](https://github.com/mmeyerlein/meclaw/issues/454)). It used to live one
level down, inside the assistant, where [#303](https://github.com/mmeyerlein/meclaw/issues/303)
had put it; #454 overtakes that placement without overturning what #303 measured.

## a level owns what its siblings must share

That is the rule the whole of GH #302 is built on and the one **ADR 0013**
states, and this level is where it stops being an abstraction. There are now
**three** concrete instances of it, and each of them is a thing two agents of one
person cannot each have their own of:

- **The memory sits here because two assistants of one person must know the same
  person.** One person, one memory. A second assistant is not seeded, not synced
  and not migrated — it starts already knowing what the member knows, because
  there was only ever one store and it was never the agent's.
- **The firewall sits here because two channels need one view of an attacker.**
  A rate window that restarts when a generation is replaced is not a rate window.
  The screen belongs to the person being screened for, not to the surface the
  attacker happened to pick.
- **The channels sit here because a bot, a screen and a phone line are the
  person's, not one generation's** (#454). A bot owned one level down is *one
  agent's* bot: the person's second agent cannot be reached through it, a
  generation swap takes the chat account with it, and a screen both agents draw
  on has no owner at all.
- **The apps sit here for the same reason the screen does**
  ([#459](https://github.com/mmeyerlein/meclaw/issues/459)). An application
  writes views onto that screen, and the screen is the person's; an app owned by
  a generation would be swept away with it, and its views would belong to
  whichever agent happened to hold it.

**The memory belongs to the member, not to the agent.** Everything else in this
template follows from that one sentence — and #454 is that sentence read one step
further: so does the way the person is reached.

## What crosses the boundary

Seven lanes in, twelve out. Every one is a lane an occupant
actually has at the version pinned above; nothing here describes a lane a holder
lost.

| in | goes to | the caller promotes |
|---|---|---|
| `in_turn` | the screen | `context.channel` — the chat or room this turn is in — and `context.user_id` if this colony has per-user firewall rules. The door edge promotes what it finds and falls back to the empty string, so an unpromoted channel costs one shared rate bucket rather than a vanished turn. A caller that wants the answer routed back into one of this member's own channels promotes `context.channel_node` as well (§ *The two channel keys*); an operator that does not gets the answer out of the level, which is what it asked for |
| `in_recall` | the memory, as its own `in_query` | `hop.recall_query`, `hop.memory_tier`, `hop.recall_window_from`, `hop.recall_window_to`, `hop.memory_call_id` (the collector's correlation id, GH #411 — promoted into context because a hop does not survive the hive), plus the round: `context.audience_set` and `context.channel`. The answer comes back on `bundle` and the refusal on `reject`, both out of this level — see § *The asker outside* ([#533](https://github.com/mmeyerlein/meclaw/issues/533)); until then the lane promised an answer the level had no exit for |
| `in_brief` | the record, to read | `context.asker` and `context.audience_set` |
| `in_propose` | the record, to write | `context.actor`, and `context.subscriber` for a `subscribe` |
| `in_build_result` | `./assistants`, under the same name | nothing. Which generation it belongs to is decided by the per-instance edge inside the container, the same way `in_bundle` finds its way home |
| `in_export` | **all three holders**, unchanged — and `./assistants` as a fourth when the caller names a generation | nothing, or `context.assistant`. All three holders declare an empty context: an export is about the whole member, never about a round. The fourth target is the exception and it is an ADDRESS rather than a round: a member with two generations has two session ledgers, so the keeper is named, never fanned to ([#475](https://github.com/mmeyerlein/meclaw/issues/475)). Since 1.4.0 the lane fans out — until [#471](https://github.com/mmeyerlein/meclaw/issues/471) only the memory answered it |
| `in_import` | the holder `hop.import_hive` names, memory by default | nothing, or `context.assistant` when `hop.import_hive` is `'session-keeper'` — that part has to reach one generation, and the container reads the same key a turn is addressed with. One export part per message, idempotent; the receipt rides the `dump` lane this level already drains (since 1.4.0, GH #467, GH #471 and GH #475) |

| out | from | what it is |
|---|---|---|
| `answer` | the record **or** an assistant | **two producers since 1.3.0.** From `./affinity` it is the brief, told apart from a push by `hop.subscriber == ''`. From `./assistants` it is a turn ANSWERED whose caller named no channel of this member — an operator, a digest, a second person's agent — carried out by a **guarded default**. An answer that *does* name a channel never reaches this lane: it is carried into `./channels` instead |
| `bundle` | the memory | the answer to a question that came in at this level's own `in_recall` door — the recalled material, on its way back to an asker OUTSIDE this member ([#533](https://github.com/mmeyerlein/meclaw/issues/533)). It leaves only for that asker: the exit is guarded on `hop.recall_caller == 'outside'`, and every other bundle goes DOWN into `./assistants` on the default edge, exactly as it always did |
| `ack` | the record | a proposed change was accepted or rejected, with its reason code |
| `reject` | the screen **or** the memory | a refusal. `hop.reject_reason` says which case. A refused RECALL is sorted like the answer to one since #533: the outside asker's leaves here, an assistant's goes back down as `in_bundle` |
| `error` | the record, a **channel**, an **app**, an assistant **or** the export sink | a failure that was not a refusal. Which cell inside produced it is not the caller's business; the member is a boundary, not a consumer. The channel source arrived with #454 — a connector's own failure used to leave through the generation that held it. The app source arrived with #459, for the same reason, and so did one more case that is not an app's at all: a screen `event` or `receipt` whose owner this level cannot place leaves here, carrying the lane it was on in `hop.kind`, rather than dead-lettering where nobody would look for it |
| `write` | an assistant | a batched conversation write, on its way past this level. It is **also** fanned onto the memory's close pass and is not consumed by that fan-out |
| `turn_write` | an assistant | one finished turn, offered for archiving as it is produced. Like `write` it is **also** fanned down — onto the memory's `in_episode` lane since #527 — and is not consumed by that fan-out |
| `prune` | an assistant | a housekeeping report, raised when something above fired `in_prune` |
| `build` | an assistant | a structural wish or a submission leaving one of this person's generations, on its way to the one baumeister the colony shares. The member neither reads it nor answers it: everything between the tool surface and the OS level is transit (GH #425) |
| `close_report` | the memory | what one close pass did to an ended session: added, sharpened, corrected, closed, restated, and the three counts that say what it could not do |
| `export_done` | `./export-sink` | one holder's seed set is complete on disk — every part written and the marker file beside them. `hop.seed_dir` says where and `hop.export_hive` which holder; three travel per export |
| `pack_ack` | an assistant | the receipt of one identity pack `./affinity` pushed into a generation. Nothing here consumes it, and nothing here can (since 1.4.0, GH #458) |

The `assistant` level emits **nine** lanes since `assistant@2.2.0`, and this
level places every one of them. The one lane it ACCEPTS that this member does not
carry, beside the four operator lanes, is `in_pack` (GH #458): its producer is
inside this level rather than above it — `<member>/affinity` is the record two
assistants of one person read — so the push edge goes from one sibling to another
and addresses `<member>/assistants/<agent>` at its own path. A lane at the
member's own door would be an interface promising something nothing outside ever
sends.

| the assistant emits | what this level does with it |
|---|---|
| `answer` | into `./channels` when `context.channel_node` names one, **out** on `answer` when it does not |
| `recall` | consumed — into the memory as `in_query` |
| `extraction` | consumed — into the memory as `in_remember` |
| `write` | **both**: fanned onto the memory's `in_close_pass` *and* out on `write` |
| `turn_write` | **both** (since #527): fanned onto the memory's `in_episode` *and* out on `turn_write` |
| `prune`, `error`, `build` | out, untranslated. Nothing here consumes them |
| `pack_ack` | **out**, untranslated (GH #458). Nothing here consumes it: affinity's own record of a delivery is the `sent_at` it writes itself, and the hive has no lane that takes a receipt — so a receipt is evidence for whoever operates the colony. Two travel per pack, one per occupant of the generation |

A level that declared a lane without the edge, or carried the edge without
declaring the lane, would be lying in one of the two directions; the pin is
`crates/meclaw-cells/tests/gh302_member_holds_the_memory.rs`
§ `every_lane_an_assistant_emits_is_consumed_here_or_leaves_the_level`, which
reads `templates/assistant/config.json` off the tree and admits no third answer.
`crates/meclaw-cells/tests/gh454_two_assistants_one_channel.rs` is the other half:
it measures that one bot really does reach two agents of one person, by name.

`error` is the sharp one, and it is why that test reads the *edges* and not only
the contract: this level already emitted `error` from `./affinity`, so the
declaration was satisfied while an **assistant's** error had no exit at all and
died as `no_route` at the container. Several senders, one lane, one declaration —
and one exit edge each.

### The asker outside, and the token that addresses it

`in_recall` is a question against this person's memory asked from **outside the
member** — an operator, a digest tool, a second person's agent. The lane shipped
with the level and its own `because` said the answer *"comes back on `bundle`
through whatever edge the caller drew"*. **There was no such edge and nowhere to
draw one:** `bundle` was not in this level's `emits`, so the answer took the one
return edge the level had, `./memory-hive -> ./assistants`, and died inside the
container as `no_route` — or, when the caller happened to carry a
`context.assistant`, was handed to a generation that never asked. The lane was a
promise this level could not keep, from the day it shipped until
[#533](https://github.com/mmeyerlein/meclaw/issues/533).

The mechanism that fixes it is the one
[#532](https://github.com/mmeyerlein/meclaw/issues/532) built (**ADR 0019**): a
reply-to token that travels up in `context.recall_caller`, is handed back on
`hop.recall_caller` by the hive's own exit, and is sorted out by the asking
side. The outside asker is simply a **third value**, and this level's door
stamps it:

| edge | lane | what it does |
|---|---|---|
| `. -> ./memory-hive` | `in_recall` | stamps `context.recall_caller = 'outside'`, beside the recall shape it already promotes |
| `./memory-hive -> .` | `bundle`, `hop.recall_caller == 'outside'` | the exit this lane never had. It **restates** its own route (`set_hop {"route": "'bundle'"}`) — a no-op for the message, and the only way `hive_contract::exit_exists` can see an exit guarded on a hop key its probe cannot carry (the GH #176 carve-out; `./affinity -> .` on `answer` is written the same way) |
| `./memory-hive -> ./assistants` | `bundle`, **default** | everything else: an assistant's own token, an unknown one, none at all. Byte for byte where every bundle went before, and the reason an unknown token is a lost answer rather than a dead letter |
| `./memory-hive -> ./assistants` | `reject`, `hop.recall_caller != 'outside'` | a refused recall of an asker INSIDE, re-stamped to `in_bundle` |
| `./memory-hive -> .` | `reject`, no token or `'outside'` | the outside asker's refusal, and every refusal of this hive that is not a recall's at all |

**The door stamps the token; it does not carry what the caller sent.** The value
space of `recall_caller` at this boundary is this level's own — a caller that
kept its own vocabulary would get its bundle routed by a word this level cannot
guard on, which is the failure again with more steps. An outside caller with
several askers of its own tells its rounds apart on `hop.memory_call_id`, which
the same door promotes into context and which crosses the hive untouched
([#411](https://github.com/mmeyerlein/meclaw/issues/411)).

**`bundle` is therefore an `emits` lane of this level**, and of the two levels
above it: `org` and `meclaw-os` carry it out, or an outside recall answered here
dies one level up instead of one level down. `params.required_drains` pairs
`in_recall` with it — *ask, and you owe the answer a drain*. That gate reads the
caller's own `set_hop` (GH #237), so a parent that names the lane in a
*condition* rather than stamping it is not caught by it; the shipped recipes are
pinned instead, in
`crates/meclaw-cells/tests/gh533_the_outside_asker_gets_an_answer.rs`.

**A refused recall now reaches the round that asked.** Before #533 every
`reject` of the memory left the level, so an assistant that asked a question the
hive would not take waited out its idle window for a bundle that never came. It
comes back as `in_bundle` with `hop.reject_reason` on the hop and the hive's own
`recall rejected: <reason>` as the body — the lane the assistant already sorts
by token and the collector already ends its memory leg on, so a refusal is a
typed result of the round rather than a second mechanism. A lane of its own
would have had to be wired by every parent, every recipe and every example for a
message that carries no new shape.

### The two refusal lanes leave, and nothing inside consumes them

Both `reject` edges are wired outward and read by nobody in this template — with
one exception since #533, and it is a refusal that HAS a reader: a refused
recall of an asker inside goes back down to that asker. Everything else leaves.
That is the honest state (2) of [#284](https://github.com/mmeyerlein/meclaw/issues/284):
**a screened-off turn with no consumer is `no_route` in the DLQ, recorded and
self-localising.** There is no `terminal` here and there will not be one — a sink
that accepts a refusal and drops it is the one arrangement in which nobody finds
out.

The holders' half of that lane is not optional in the same loose sense: all
three declare `required_drains` pairings against `reject` for the ingresses this
level sends them — `in_episode`, `in_query`, `in_remember`, `in_close_pass`,
`in_export` and `in_import` at the memory, `in_export` and `in_import` at the record and at the
screen since [#471](https://github.com/mmeyerlein/meclaw/issues/471). Whoever
wires a member drains its `reject`, and `./affinity -> .` on that lane is new in
1.4.0 for exactly that reason: the record had no refusal to raise before it had
a transfer that could refuse one.
`close_report` is the same obligation under another name: the hive pairs
`in_close_pass` with it, so a member that fires the close pass and whose parent
does not take the receipt is refused at the mutation, not at the message.

## The wiring, and why each edge exists

**The screen, and its two feeds.** A turn reaches the firewall from exactly two
places, and since 1.3.0 the second one is the interesting half:

- `. -> ./firewall` — a turn injected at this member's own door, from an
  operator, a digest or a second person's agent.
- `./channels -> ./firewall` — a raw turn one of *this person's* channels
  raised, re-stamped from `turn` to `in_turn`. This edge is byte-identical to the
  one that used to read `./assistants -> ./firewall`: same condition, same
  `set_hop`, same `channel` promotion. Only its sender changed, and that is the
  whole mechanical content of #454 at this level — **the raw wire no longer
  crosses the generation.**

The screened turn comes back on `pass`, re-stamped to `in_turn` again, and
`./assistants` routes it to the generation it was addressed to by an edge the
instantiating mutation drew. Both edges out of the firewall clear its own context
keys (`fw_body`, `fw_now`, `fw_phase`, `store_origin`), because the parked copy
of the turn rides along otherwise — that is the firewall's own instruction, not a
precaution added here.

**The answer, and the guarded default.** `./assistants -> ./channels` carries the
answer back when `context.channel_node` names something; `./assistants -> .` carries
it out of the level when it does not, and that edge is a **guarded default**
([#283](https://github.com/mmeyerlein/meclaw/issues/283)), not a second
unconditional exit. Exactly one of the two fires. Suppression is per sender, so
the rule that keeps it honest is the same one the assistant level lives under:
every *other* regular out-edge of `./assistants` is conditioned on a route an
`answer` message does not carry, and there is no unconditional tee. A tap added
to that container without its own route condition would silence the default and
strand every channel-less answer.

**The memory: one pair that reads, two edges that write.** `recall` → `in_query`
and `bundle` → `in_bundle` are the pair the whole level exists for: one memory,
every assistant of this member reading it — and, since [#532](https://github.com/mmeyerlein/meclaw/issues/532), both askers
*inside* one of them. The reply-to token that sorts the answers out crosses this
level twice and is read neither time: it goes up in `context.recall_caller` and
comes back on `hop.recall_caller`, put there by the memory hive's own
exit. Nothing here promotes it, deletes it or knows what its values mean —
except at this level's OWN door, where the same token names the third asker
(§ *The asker outside*). The write half is **two** edges, and
they write two different things:

- `extraction` → `in_remember` writes the **facts** the front model annotated
  inside its own answer. It is the second half of the recipe `talky` prescribes
  to its parent (*two edges, never one*,
  [`../talky/README.md`](../talky/README.md) § the extraction sidecar); the first
  half is inside the talky, and the drain the recipe asks for is the `reject`
  lane above.
- `turn_write` → `in_episode` writes the **turns**, one message per turn, and
  since [#298](https://github.com/mmeyerlein/meclaw/issues/298) it is the only
  path in the substrate from a conversation into an `episodes` table. It arrived
  here with [#527](https://github.com/mmeyerlein/meclaw/issues/527) and it is a
  **fan-out**, like the close pass below: the same turn still leaves the level on
  `turn_write`, because the archive above and the memory below want the same
  event for different reasons.

The order between the two is not a preference. `in_remember` **presupposes**
`in_episode`: an annotated block names no turn, so the ingress binds it to the
newest `user` episode of the session — with no episodes there is nothing to bind
to and the hive refuses every block, which is the failure
[`../talky/README.md`](../talky/README.md) names in one line. Until #527 this
level declined `turn_write` and pointed at `extraction` as "the member's own
episode path"; that sentence was wrong, both lanes it pointed at read the table
it had declined to fill, and every stored turn dead-lettered as `hive_no_route`
at the OS root while the collector stamped it `episode_written = 1`.

**`turn_id` comes off the hop.** `context.turn_id` is a round uuid;
`hop.turn_id` is the deterministic `<session_id>#<index>` the collector mints,
and it is what the inline bind and the queue row are keyed on. An edge that
promoted the context key would produce episodes nothing can later bind to — a
defect that looks exactly like the missing edge from the outside.

**The `recall` edge promotes it too, and that took until
[#535](https://github.com/mmeyerlein/meclaw/issues/535) to show.** A hive forms its own
hop (GH #411), so context is the only compartment that survives one — and the collector's
`in_bundle` lane parks the whole turn when the bundle comes home unable to name the round
it belongs to. `talky`'s `recall` exit has named `turn_id` among its keys all along; this
level promoted seven of them and not that one. It never showed on the TOOL path, which is
why it shipped: a `memory_recall` call happens *after* the brain call and the brain edge
has promoted `hop.turn_id` into context long before. The AMBIENT leg leaves before the
model has seen the turn, so there is no context copy yet — measured on a running colony
the moment the ambient knob was turned on, as silence.

**The record, and the subscription it owns.** Read on `in_brief`, write on
`in_propose`, answers back out. And the **push** into a subscribing brain belongs
here: that was the half [#302](https://github.com/mmeyerlein/meclaw/issues/302)
left open and it is decided ([#453](https://github.com/mmeyerlein/meclaw/issues/453)).
A subscription is a **row in the record**, the record is this level's, and an
assistant is replaced per generation — a subscription owned one level down would
have to be re-written on every swap, and the brain that comes up after it would be
silent until somebody remembered. So the level that holds the record holds the
subscription.

Two things follow, and they are not the same thing:

- **The row ships.** The record ships one active subscription at birth (the seeded
  agent's own document into the seeded agent's own brain — `seed/subscribers.jsonl`
  over in [`../affinity/README.md`](../affinity/README.md) § *The seed*), so the
  push lane carries traffic on the first tick of a fresh member instead of being
  silent for a structural reason nobody can tell from *nothing changed*.
- **The edge does not, and cannot.** Writing an edge is a mutation and mutation
  authority is the colony's — a `subscribe` that could wire its own delivery would
  be a cell granting itself a route. So the two edges are drawn by the mutation
  that instantiates this level, one per subscribing cell, and the recipe is
  [`../affinity/README.md`](../affinity/README.md) § *Wiring `out_push` for a
  subscribing brain*; the seeded row's `cell_path` is the token that mutation
  either conditions the edge on or rewrites through the record's own write port.
  A subscription with no edge behind it is accepted and undeliverable — that is
  the parent's bug to avoid, not something this level can refuse for it.

What this level *does* ship of the recipe is its other half: the `answer` exit
from the record is conditioned on `hop.subscriber == ''`, so a push can never
leave on the brief lane and land on a caller that never asked
([#289](https://github.com/mmeyerlein/meclaw/issues/289)).

**The close pass.** One edge, no new cell: `./assistants -> ./memory-hive` on
`hop.route == 'write'`, re-stamped to `in_close_pass` and promoting
`session_id`, `audience_set` and `channel` off the context the close batch
already carries. It is a **fan-out**, not a redirection — the same batch still
leaves the level on `write`, because the archive above and the memory below want
different things from the same event. The receipt comes back on `close_report`
and leaves: this level fires the pass and does not read it
([#447](https://github.com/mmeyerlein/meclaw/issues/447)).

**The export, and the one cell this level owns.** `in_export` crosses the
boundary untouched onto the `in_export` of **all three holders at once** —
`./memory-hive`, `./affinity` and `./firewall` — and, when the caller names a
generation, onto a **fourth** target: `./assistants`, through which the demand
reaches that generation's own session keeper four levels down
([#475](https://github.com/mmeyerlein/meclaw/issues/475)). Every part any of
them walks out comes back on `dump`, into `./export-sink`. That cell is a `code` cell
that files each part under the hive it came out of:
`<MEMBER_EXPORT_DIR>/<hive>/seed/<table>.jsonl`, the schema declaration as line
1, one row per line after it, which is the birth format
[`../memory-hive/README.md`](../memory-hive/README.md) § *The document*
specifies and the two other holders' stores read the same way. An `absent` table
writes no file; an empty one writes a file with only its schema line, because
those are two different statements. The last part of a hive also writes
`<hive>/seed/export_final.json`, rewrites the member-level
`export_final.json` that names every holder finished so far, and the level says
`export_done` with `hop.export_hive` naming which one. **Three travel per
export**, in whatever order the walks finish in — **four** when a generation was
named and its keeper walked its ledger out beside them.

**The fourth target is guarded, and the guard is the point.** The edge reads
`hop.route == 'in_export' && has(context.assistant) && context.assistant != ''`.
Two measurable reasons, neither of them taste. A member with two generations
holds **two** session ledgers and they are not one document; and the sink files a
part under the hive it came out of, so two keepers would both claim
`<MEMBER_EXPORT_DIR>/session-keeper/` and the directory would hold whichever walk
finished last, silently. An export that names no generation is therefore exactly
the export this level always did — three holders, no keeper, and no dead letter,
because the container is open and an unguarded fan-out into an empty one would
be a `no_route` on every export a member without an assistant ever ran.

**Until [#471](https://github.com/mmeyerlein/meclaw/issues/471) only the memory
hive answered.** That is worth stating as a retraction rather than as a feature,
because the export said `member` on the tin the whole time: a colony grown from
one reproduced the memory completely — every episode, every fact, every
embedding — and reproduced the record and the screen as empty tables. `memory`
produces and `affinity` decides, so a member reborn like that remembered
everything it had been told and knew nothing about who may be told what, and it
screened its first inbound turn against no rules at all. That is not a smaller
backup; it is a different security posture wearing the same name. The fix is
additive and it is each hive's own: [`affinity`](../affinity/README.md) and
[`firewall`](../firewall/README.md) each grew a porter of their own on the lane
`memory-hive` has had since 2.2.0, and this level fans out to them and drains
all three.

Four details there are decisions rather than defaults. **One directory per
hive** is a requirement, not tidiness: `memory-hive` and `affinity` both declare
a table called `entities`, so the flat sink this level shipped before #471 would
have written one over the other without a word. Each part names its own hive
(`hive_template`) and the sink files it under that name. **The `dump` edges test
`hop.route` and nothing else**: every one of the three hives pairs `in_export`
with `dump` in `params.required_drains`, and the probe that checks the pairing
runs the described hop through the real edge evaluator, so an edge additionally
guarded on `hop.dump_kind` evaluates false under it and reads as no drain at
all — one plain edge per holder, three in total. **The sink is a `code` cell
rather than a `file` cell** because a `file` cell canonicalises its `base_path`
in `validate_params` — a member whose export directory did not exist would fail
`--validate` and boot, for a lane nobody had used yet; the sandbox write root is
the same boundary one message later, where a missing directory is a routed
`io_error`. And `params.max_concurrency` is `1`, so the parts are written in
walk order and a marker cannot be written before the file it claims is complete.

**What a holder leaves behind is the holder's decision, not this level's.**
`memory-hive` keeps its three machine tables and its `emb_models` configuration
(§ *What travels, and what deliberately does not* there); `affinity` blanks
`subscribers.pack_hash` and `sent_at` on the way IN, because they record what
the SOURCE already delivered to a cell path in a colony that no longer exists,
and a reborn member that inherited them would never make its own first identity
delivery; `firewall` leaves `arrivals` behind, because a rate window is the
budget one installation spent and a colony that inherited a full one would
refuse turns for traffic it never saw. This level reads none of that and
decides none of it.

Pinned end to end in
`crates/meclaw-cells/tests/gh471_a_member_carries_all_of_itself.rs`: two real
colonies sharing nothing but a directory, one distinctive row per holder, and
the receiving colony's firewall refusing a turn on a rule that only ever existed
in the sending one.

### The import: a birth and a lane, and they are not the same act

This level has `in_import` **since 1.4.0**
([#467](https://github.com/mmeyerlein/meclaw/issues/467)): one accepted lane and
one plain edge onto `./memory-hive`. That lane is the *second* step, and saying
which step is which is the whole of this section — a memory arrives one of two
ways, and only one of them is a message.

**At birth, as a seed.** The parts this level writes out ARE seed files, so a
member can be grown with them already inside it. The obstacle is that
`./memory-hive` is a `ref` and a reference carries no files: the only manifest
key that carries files is `add_templates[].files`, so the reference has to be
written out into a derived template first, and that template registered and
instantiated in the SAME diff. That is one manifest, and
[`examples/memory-import/`](../../examples/memory-import/) is it. The order is
not negotiable — a seed is read when the `cell.db` is created and is inert for
ever after, so a member that is already running cannot be given a past.

**Afterwards, as a message.** Everything the source learned or decided since the
export was walked reaches a hive that is now running, and all three holders
accept `in_import` for that. The door THROUGH this level is what 1.4.0 adds:
the lane, and the edges that carry it untouched — plain, the way `in_export`'s
are, because `in_import` is the name on both sides of this boundary. It needs no
new drain: an import receipt rides the `dump` lane this level already takes into
`./export-sink`, and the sink ignores it on purpose.

**Which holder a part belongs to is edge truth.** A body is model-writable and
an edge is not, so the holder is read off `hop.import_hive`: `'affinity'` and
`'firewall'` each have their own guarded edge, and `./memory-hive` is the
**guarded default** ([#283](https://github.com/mmeyerlein/meclaw/issues/283)) —
the router evaluates it only when no guarded edge decided. That is not a
courtesy to lazy callers; it is what keeps every part written before
[#471](https://github.com/mmeyerlein/meclaw/issues/471) arriving where it always
arrived, since the memory hive was the only place it could have come from.
`'session-keeper'` is the third guarded name and it is the odd one out: its edge
goes to `./assistants`, because the hive it names stands four levels below the
deepest endpoint this level can address ([#475](https://github.com/mmeyerlein/meclaw/issues/475)).
Which generation's keeper a part lands in is then the container's own question,
answered on `context.assistant` — and a part that names the keeper and no
generation has no address at all, so it stops as `no_route` at
`<member>/assistants` rather than being handed to a holder that would refuse it
under some other name. **Four** doors, exactly one of which can fire per
message.

For one release the lane lived only on the derived template
[`examples/memory-import/`](../../examples/memory-import/) built, so a member
grown the ordinary way had no second step at all. It ships here now, and the
example copies it like every other line of the level. **The birth is still the
example's job**: a `ref` carries no files, so putting a seed into `./memory-hive`
takes an `add_templates` and cannot be done by a lane. Pinned in
`crates/meclaw-cells/tests/gh467_the_shipped_member_carries_the_import_lane.rs`
(the shape of the shipped file) and
`crates/meclaw-cells/tests/gh467_a_member_is_born_with_its_history.rs`, which
grows a member out of another colony's export and then feeds the running one a
later document. The four-door shape of the lane is pinned in the first of those
two, and the keeper's own leg of it in
`crates/meclaw-cells/tests/gh475_a_member_reaches_the_keeper_it_holds.rs`.

**The three that only pass through.** `prune`, `build` and an
assistant's `error` get one plain exit edge each, `./assistants -> .`, and no
translation on the way. `turn_write` was the fourth until #527 and is not one
any more: it leaves on that same plain edge **and** is fanned onto the memory's
`in_episode`, exactly the way `write` is fanned onto the close pass. An assistant's `dump` is not one of them and never was:
it is the transfer document of that generation's session keeper, and this level
**consumes** it, into the same `./export-sink` its own three holders write to
(#475). This level owns no archive and no timer, so it has
nothing to do with any of them except refuse to swallow them — which is the
same rule the refusal lanes follow, applied to lanes that are not refusals.
`write` and `turn_write` are the two fan-outs: each leaves on the same kind of
plain edge *and* is fanned into the memory hive, which is why neither is in that
list.

### Identity comes from the edge, at every door

Since [#291](https://github.com/mmeyerlein/meclaw/issues/291) a `context` key a
hive lane declares is **enforced**: an edge that states that lane must promote
the key itself or have a setter reachable upstream. At this level `.` is the
door and nothing is upstream of it, so **the edge is the only setter root**, and
every edge here that stamps a lane into a holder promotes what that lane asks
for.

Each promotion is written `has(...) ? ... : ''` rather than as a bare read. A
modifier that fails to evaluate skips the whole edge, and a turn that vanishes on
an edge is invisible; a turn that arrives with an empty key is refused **by the
holder**, on the `reject` lane this level already drains, with a reason. Empty
string means unset in all three holders, which is what makes that trade legal.

The round has exactly one spelling: **`audience_set`**. `participants` is
**retired, not aliased** ([#330](https://github.com/mmeyerlein/meclaw/issues/330))
— a request that spells the round that way declared no round at all and is
refused like any other undeclared one. No template may introduce a second name
for it.

## The `channels` container

`channels` is a real, empty, **open** hive beside `assistants`, and it ships with
no cells, no ports and no contract — for the same two reasons the other container
does (see *Why a container carries no contract* below).

**One channel is one node in it,** named for what `context.channel_node` will
say: `telegram`, `slack`, `display-<screen>`, `voice`. For a chat channel that
node is the connector cell itself — `telegram-connector` is a `proxy` and needs
no hive around it. The name is not a label: it is the value the turn carries up
and the value the answer is routed back by, so it is a fact of the wiring.

### The two channel keys

**`channel_node` is the address. `channel` is the conversation.**
([#522](https://github.com/mmeyerlein/meclaw/issues/522).) They used to be one
word, and the word could only be one of the two things:

| key | what it holds | who reads it |
|---|---|---|
| `context.channel_node` | the **node name** in this container — `telegram`, `display-desk` | the edges. `Edge.to` is a static path and a container may hold several channels, so every way back says which child it is for |
| `context.channel` | the **chat**: whatever the surface calls the same conversation partner — a Telegram `chat_id`, a room, a phone number | the holders. `session-keeper` opens one generation per value, `firewall` rate-limits one bucket per value, `memory-hive` writes it down as the room a thing was said in, and its audience gate has a channel-local clause over it |

Written as one key it had to be the node name, or no answer routed anywhere —
and then every chat of one connector shared **one** session generation, **one**
rate bucket and **one** room: the idle clock, the nightly close and the session
id were computed over the union of all of them. Split, each half says the thing
it was always read as saying. A **screen** carries the same word in both,
because a screen is one room; a **chat connector** does not, and that is the
whole of the repair.

Both are promoted on the channel's own ingress edge, and both are written
`has(...) ? ... : ''` where they come off the hop — a modifier that fails to
evaluate skips the whole edge.

**For a chat channel: two lanes up, one lane down, and no more.** (A screen is a
channel too and carries more — see *The display channel* below.)

| direction | lane | who ships the edge |
|---|---|---|
| up, from the channel | `turn` — a raw inbound message | the channel's mutation draws `./channels/<name> -> ./channels`; **this level** ships `./channels -> ./firewall`, which re-stamps it to `in_turn` |
| up, from the channel | `error` — the connector's own failure | the channel's mutation draws it; **this level** ships `./channels -> .` |
| down, to the channel | `answer` — what an assistant said | **this level** ships `./assistants -> ./channels`; the channel's mutation draws `./channels -> ./channels/<name>`, guarded on `context.channel_node == '<name>'` |

A connector emits **one wire**: an emission carrying `hop.error_code` is its own
failure, one without it is an inbound turn. Normalising the two onto `turn` and
`error` is the parent's job, and the two upward edges are that normalisation.
`telegram-connector`'s own README puts the `error` **drain** on the level that
holds it — the obligation #303 addressed to `channels` and #454 moved here with
the container. This level ships the exit edge (`./channels -> .`) and declares
`error` on its contract, but it declares **no `params.required_drains`** of its
own, so the obligation still travels in prose: a parent that wires a member and
leaves its `error` lane unwired gets `no_route` in the DLQ, not a refused
mutation. The outbound edge must promote whatever the connector needs to reply — for Telegram,
`hop.chat_id` into context, or the reply has no chat to go to. One `getUpdates`
consumer per bot token: a second poller on the same token gets 409 and the two
steal each other's updates.

**Adding a channel costs one node and three edges, and no template moves.**

```json
{"scope": "<member>", "diff": {
  "add_nodes": [{"name": "channels/telegram", "template": "telegram-connector@2.0.1"}],
  "add_edges": [
    {"from": "./channels/telegram", "to": "./channels",
     "condition": "!has(hop.error_code)",
     "modifier": {"set_hop": {"route": "'turn'"},
                  "set_context": {"channel_node": "'telegram'",
                                  "channel": "has(hop.chat_id) ? hop.chat_id : ''",
                                  "assistant": "'scribe'",
                                  "chat_id": "has(hop.chat_id) ? hop.chat_id : ''",
                                  "user_id": "has(hop.user_id) ? hop.user_id : ''"}}},
    {"from": "./channels/telegram", "to": "./channels",
     "condition": "has(hop.error_code)",
     "modifier": {"set_hop": {"route": "'error'"}}},
    {"from": "./channels", "to": "./channels/telegram",
     "condition": "has(hop.route) && hop.route == 'answer' && has(context.channel_node) && context.channel_node == 'telegram'"}
  ]
}}
```

The mutation is scoped to the **member**, not to the container: a node is
addressed by its `name` plus the scope and the name carries the `/`, endpoints
are scope-relative always, and scoping to `<member>/channels` would refuse an
absolute endpoint with `scope_out_of_bounds` and `"to": "."` with `edge_schema`.

### The display channel

`./channels/display-<screen>` is where a **screen** stands, and it is wired
exactly like a chat channel. The screen is a channel because that is what it
behaves like, and it is a channel **of the person** — which is precisely why two
of their agents may hold views on it at the same time. A screen owned by a
generation would go dark on a swap and could not be shared at all.

Since GH #459 the cell that stands there is real: [`display@1.0.0`](../display/).
Three edges instantiate one, and only the third of them says anything a chat
channel's edges do not:

| edge | condition | why |
|---|---|---|
| `./channels/display-<s> -> ./channels` | `event` or `receipt` | what the screen produced, stamped with `context.channel_node` and `context.channel`, which on a screen are the same word |
| `./channels/display-<s> -> ./channels` | `has(hop.error_code)` | the screen's own failure, re-stamped to `error` |
| `./channels -> ./channels/display-<s>` | `answer` or `view`, `context.channel_node == '<s>'` | re-stamped to the display's own `in_view` |

**The smallest view needs no app.** An agent's ordinary `answer` becomes a view
through that third edge — the same `./assistants -> ./channels` lane GH #454 drew
for a chat answer carries it, and nothing at this level knows the difference. An
agent that only wants to show a paragraph does not have to become an application
first, which was the half of GH #455 that had nowhere to live.

### The way back: `event` and `receipt`, routed by owner

A screen produces two lanes nothing else produces: `event` (a person did
something on a view) and `receipt` (a write was refused). Both are addressed to
the **owner** of the view — the path of the cell that put it up, taken from
`envelope.reply_to` and never from the body — and the display stamps that path on
`hop.owner`, with `hop.view_id` beside it.

It has to be the **hop**. An edge condition in this substrate is evaluated
against `context.*` and `hop.*` and nothing else
(`crates/meclaw-colony/src/cel_eval.rs`, `bind_ctx`), so an owner that lived only
in the body could not be routed on at all.

This level splits on the **container** and never on the agent:

| owner path contains | goes to | as |
|---|---|---|
| `/assistants/` | `./assistants` | `in_turn`, with `hop.kind` set to `event` or `receipt` |
| `/apps/` | `./apps` | `event` / `receipt`, lane name kept |
| neither, or empty | `.` | `error`, with the original lane on `hop.kind` |

Two things about that table are deliberate.

**It matches with `contains`, not with a prefix.** The owner is an *absolute*
cell path and a template does not know its own absolute prefix. `contains('/assistants/')`
is the prefix test a level-relative template can actually write.

**An agent gets it as `in_turn`.** The `assistant` level accepts no event lane
and did not grow one for this: `in_turn` is the lane it has, and `hop.kind` is what
tells a brain that this turn came from a button rather than from a keyboard.
`context.channel_node` travels with it, so the answer finds its way back to the
same screen.

The third row is the one that keeps a defect visible. The display emits an event
whose object id will not parse **anyway**, with an empty owner, because a view it
holds and cannot attribute is something a person has to see — and that is only
true if somebody does see it. The member re-stamps it onto the `error` lane it
already emits, so no new exit is owed to the parent.

**Which agent and which app is the mutation's edge**, exactly as under GH #454:
`Edge.to` is a static path, so the per-assistant edge grows one clause —
`context.assistant == '<name>' || hop.owner.contains('/assistants/<name>/')` —
and an app costs the mirror pair. One edge per recipient per direction, a sum and
never a cross product.

## Addressing an assistant through a channel

**Rule v1, GH #454.** One channel may deliver to **several** assistants of the
same member, and which one a message was meant for is decided by **edges**, never
by a model.

**1. The channel stamps the name.** The channel's outbound edge —
`./channels/<name> -> ./channels`, the one that raises `turn` — also stamps
`context.assistant` with the name of the agent the message was addressed to. It
gets that name from one of exactly two places:

- **the channel's address rule** — a prefix or a mention that the connector
  parsed onto a hop key (`"assistant": "hop.addressed_to"`), where the connector
  has such a rule at all. `telegram-connector` has none today: it emits
  `platform`, `chat_id`, `user_id`, `message_id` and `msg_type`, and nothing that
  names an agent;
- **the channel's default**, written as a **literal in that very edge**
  (`"assistant": "'scribe'"`), which is what the snippet above does and what
  every message on a channel with no address rule uses.

Both together are one expression, and it is still an edge:

```json
"set_context": {
  "assistant": "has(hop.addressed_to) && hop.addressed_to != '' ? hop.addressed_to : 'scribe'"
}
```

**2. The container fans out.** `./assistants` routes on that key with **one edge
per assistant**:

```json
{"from": "./assistants", "to": "./assistants/scribe",
 "condition": "has(hop.route) && hop.route == 'in_turn' && has(context.assistant) && context.assistant == 'scribe'"}
{"from": "./assistants", "to": "./assistants/coach",
 "condition": "has(hop.route) && hop.route == 'in_turn' && has(context.assistant) && context.assistant == 'coach'"}
```

**Why one edge each, and not one dynamic edge:** `Edge.to` is a static `Path` in
the substrate (`pub struct Edge` in
`crates/meclaw-colony/src/edge_table.rs`). There is no edge that means *"send it
to whatever `context.assistant` says"*, and there will not be one — an edge whose
target is computed from message content is a route a message granted itself. A
new assistant therefore costs **one edge per direction**, drawn by the mutation
that instantiates it. That is a documented rule, not an accident.

**3. The way back is symmetric, and it is per CHANNEL.** The assistant emits
`answer`, this level carries it into `./channels`
(`./assistants -> ./channels`, guarded on `context.channel_node != ''`), and
inside the container **one edge per channel** decides who replies:

```json
{"from": "./channels", "to": "./channels/telegram",
 "condition": "has(hop.route) && hop.route == 'answer' && has(context.channel_node) && context.channel_node == 'telegram'"}
```

`context.channel_node` rode in on the turn and rides back out on the answer; the
assistant never learns which channel it was and must not. `context.channel` rides
along beside it and says which CHAT — that is the half the holders read, and
since [#522](https://github.com/mmeyerlein/meclaw/issues/522) it is a separate
key for exactly that reason (§ *The two channel keys*).

**4. The transfer lanes are addressed the same way (#475).** An export or an
import part for a generation's session keeper reaches `./assistants` from the
member's own door, and the container decides which generation with the same key
and the same shape:

```json
{"from": "./assistants", "to": "./assistants/scribe",
 "condition": "has(hop.route) && (hop.route == 'in_export' || hop.route == 'in_import') && has(context.assistant) && context.assistant == 'scribe'"}
{"from": "./assistants/scribe", "to": "./assistants",
 "condition": "has(hop.route) && hop.route == 'dump'"}
```

The `dump` edge is **plain** and has to be: every level between here and the
keeper pairs `in_export` with `dump` in `params.required_drains`, and the probe
that checks the pairing runs the described hop through the real edge evaluator —
so an edge that additionally tested `hop.dump_kind` would evaluate false under it
and read as no drain at all. What arrives on it are the parts of that keeper's
document and the receipts of applied ones, and this level files both in
`./export-sink`.

**5. And the member itself is addressed the same way (#478).** One storey up,
`./members` fans out to its children exactly as `./assistants` does here — one
edge per member, guarded on `context.member`. The only difference is the FORM:
`context.assistant` is strict (`has(…) && … == 'scribe'`) because a turn that
names no generation has nowhere to go, while `context.member` is permissive
(`!has(…) || … == 'alex'`) because nothing promotes it yet and a strict guard
would strand every turn a running colony has. Both are the same rule: `Edge.to`
is static, so a container with two children costs two edges and each one says
which child it is for.

**So the cost is N + M, not N × M.** *N* assistants cost *N* addressing edges in
`./assistants`, *M* channels cost *M* reply edges in `./channels`, and no edge
anywhere names a pair. Two assistants on one channel is three addressing edges
in total — and neither assistant's template mentions the channel, nor the
channel's node either assistant.

### One member, two assistants, one channel

The whole arrangement, as three mutations. The member first:

```json
{"scope": "<org>/members", "diff": {
  "add_nodes": [{"name": "alex", "template": "member@1.4.0"}]
}}
```

then one mutation per assistant — the addressing edge plus the thirteen transit
lanes (`../assistant/README.md` § *Instantiating* writes them out):

```json
{"scope": "<member>", "diff": {
  "add_nodes": [{"name": "assistants/scribe", "template": "assistant@2.2.0"}],
  "add_edges": [
    {"from": "./assistants", "to": "./assistants/scribe",
     "condition": "has(hop.route) && hop.route == 'in_turn' && has(context.assistant) && context.assistant == 'scribe'"},
    {"from": "./assistants/scribe", "to": "./assistants",
     "condition": "has(hop.route) && hop.route == 'answer'"},
    {"from": "./assistants", "to": "./assistants/scribe",
     "condition": "has(hop.route) && (hop.route == 'in_export' || hop.route == 'in_import') && has(context.assistant) && context.assistant == 'scribe'"},
    {"from": "./assistants/scribe", "to": "./assistants",
     "condition": "has(hop.route) && hop.route == 'dump'"}
  ]
}}
```

and the same again for `coach`, then the channel mutation from *The `channels`
container* above. Nothing has to be re-run afterwards: `coach` starts already
knowing what the member knows — the same memory, the same record, the same rate
window on the same attacker — and is reachable on the same bot, by name. Nothing
is copied and nothing is synchronised, because there was only ever one of each.
That is what #454 bought, and
`crates/meclaw-cells/tests/gh454_two_assistants_one_channel.rs` is what measures
it.

## The containers

`assistants`, `channels` and — since GH #459 — `apps` are real, empty, **open**
hives. Open because the
mutation that instantiates an assistant, a channel or an app draws edges to that node,
and a sealed hive refuses exactly those endpoints with `hive_port_boundary`. They
ship with no cells and no edges of their own; the member wires them, and each
instantiation wires itself.

**Their unbound behaviour is undeclared.** GH #285's slot governs an address that
does **not** exist, and these containers do exist — so the declared word could
never fire, and a message that reaches one of them before anything is
instantiated takes the ordinary path. The measurement comes from
`unbound_slot_behaviour` in `crates/meclaw-colony/src/colony.rs`, which steps
aside as soon as the target is a registered hive scope. Writing `params.ports`
for a slot's sake would additionally **seal** the member, which is the opposite
of what a level that gets wired into is for. No hive in this template carries a
`ports` key.

**What transits `./assistants`**, derived from the contract of `assistant` and
from what this member sends back down (`firewall`, `memory-hive`), each at the
version its `because` names:

- **in** — `in_turn` (the screened turn, carrying `context.assistant`) and
  `in_bundle` (the memory's answer). Both are produced by a sibling of the
  container, not by a caller outside the member. `in_build_result` crosses from
  the member's own door and is delivered here as well, and since #475 so do
  `in_export` and `in_import` — the transfer lanes of the generation's session
  keeper, the only two that name a generation with `context.assistant` at the
  member's own door rather than at a channel's.
- **out** — the **nine** an assistant emits: `answer`, `write`, `turn_write`,
  `extraction`, `recall`, `prune`, `error`, `build` and — since #475 — `dump`,
  the only one of them this level consumes rather than re-emits.

**What transits `./channels`** — `turn`, `error`, `event` and `receipt` up;
`answer` and `view` down. The container reads nothing in any of them: it carries
the message, and the address rule that decided where it goes lives in the edge
below it or in the owner the screen stamped.

**What transits `./apps`** — `view` and `error` up, `event` and `receipt` down.
An **app** is a specific composition and specific code that came out of a build
order, a derived template in the library tagged `app`, instantiated here as
`apps/<name>`. It stands beside the agents rather than inside one because it
belongs to the *person*: it outlives a generation swap the way a channel does,
and the screen it writes to is the person's too.

**An app has no port and no surface of its own. It writes views.** Whatever
authentication stands in front of the screen stands in front of the app; giving
an app a port of its own would be a second front door nobody counted. An app is
also display-*blind*: it emits `view` and never names a screen. Which screen it
draws on is one literal, in the edge that leaves the app —
`set_context.channel_node: "'display-<screen>'"`, with `channel` carrying the
same word — which is why
[`colony-view`](../colony-view/) can be wired to two displays without knowing
either of them.

### Five inbound lanes this level deliberately does not carry

The `assistant` level accepts **ten** lanes. Five of them cross this level:
`in_turn` (handed down by the screen), `in_bundle` (handed down by the memory),
`in_build_result` (which enters at the member's own door and is forwarded) and,
since #475, `in_export` and `in_import` (which enter at the same door and are
forwarded the same way). The other five — **`in_advice`**, **`in_sweep`**,
**`in_prune`**, **`in_round_sweep`** and **`in_pack`** — are **not** lanes of
this member, and that is a decision rather than an omission (orchestrator ruling
W7-R5).

A level's transit contract carries the lanes that *cross* it. An emitted lane
always crosses: it is produced inside and has to get out, which is exactly why
the outward ones above are here. An accepted lane crosses only when its producer
sits **outside** the level and addresses **through** it. These four do not:

| lane | who produces it |
|---|---|
| `in_advice` | `./cogny`, inside the assistant. The other producer is a second agent, which stands beside the first in this same container. |
| `in_sweep` | an operator. The assistant's own `because` says it *"enters at the assistant path rather than being produced by a sibling"*. |
| `in_prune` | a timer or an operator — paired with the `prune` report the member *does* carry outward. |
| `in_round_sweep` | the same owner as `in_sweep`, entering the same way. |
| `in_pack` | `<member>/affinity`, a **sibling** of the container (GH #458). Producer and consumer are both inside this member, so the push edge is drawn from one to the other and addresses `<member>/assistants/<agent>` at its own path. A lane at this level's own door would promise something nothing outside ever sends. |

They reach the assistant at its own address, `<member>/assistants/<agent>`, and
they may: neither this level nor the assistant declares `params.ports`, so both
are **open**, and the port boundary forbids an outside endpoint below a hive path
only for a *sealed* hive
(`crates/meclaw-colony/src/mutation/port_boundary.rs`). Declaring them here would
promise a road nobody drives on — the mirror image of the outward lanes that had
no road at all.

The exception is pinned, not merely written down:
`gh302_member_holds_the_memory.rs`
§ `the_lanes_an_assistant_takes_from_an_operator_deliberately_do_not_cross_this_level`
lists exactly these five and requires every *other* lane the assistant accepts to
be supplied from inside. A sixth lane that really does arrive from above goes red
there; carrying one of these five later is a deliberate edit of that list, this
paragraph, and the `org` and `meclaw-os` contracts with it.

### Why a container carries no contract

Both transit lists are prose in the containers' own `description`, not a
`params.contract`, and the reason is mechanical rather than stylistic.
`addressed_lane_doors` skips a hive only while **nothing addresses its path**
(`hive_path_is_wired`). This member addresses `./assistants` on nineteen of its
edges and `./channels` on nine, so both containers are wired the moment the
member is instantiated — and from then on every lane they declared would owe a
`door_exists`: a message arriving at the container path must reach a cell
*inside* it. An empty container has no inside. The violation would be collected
on **every** mutation of the colony, not only on one that touches this member, so
a contract here would lock the colony for exactly as long as this member has no
assistant — or no channel — yet, which is a perfectly ordinary intermediate
state, and the normal one for `channels` on a fresh member.

The rule, which holds for all four levels: **a container hive that its own level
wires declares no `params.contract`. The transit lanes are declared by the level
whose own edges satisfy the door and exit check from birth.** A container nobody
wires could technically carry a dormant contract; it should not — a declaration
that is green only because nothing is looking is the same defect class as the
slot this wave struck.

A container is an address. An address is not an interface until something stands
at it.

## What is deliberately not here

- **No agent and no tool surface.** Both are the assistant's, and one member may
  own several.
- **No connector cell in the template.** A channel is instantiated, never
  shipped: `./channels` ships empty, and what stands in it is a mutation's doing.
- **No address rule in the container.** Which agent a turn was meant for is
  decided by the channel's own outbound edge and by the per-assistant guards.
  `./channels` and `./assistants` both carry the turn and read nothing in it.
- **No `memory-drain`.** Per-turn extraction ([#298](https://github.com/mmeyerlein/meclaw/issues/298),
  ruling Q11) replaced it, and #302 says explicitly that it does not belong in
  the assistant either.
- **No `terminal`, and no sink for a refusal.** See the refusal lanes above.
  `./export-sink` is not that kind of sink: it is the declared destination of one
  lane whose whole point is to land on disk, it reads nothing else, and it
  swallows no refusal.
- **No org-level anything.** A group is an audience, not a holder: *what does the
  group know about X* is a filter on the read, never a second store. Two stores
  would force the writer to pick one before extraction has run, which is not a
  decision it can make. A group that owns an agent nobody owns personally is a
  **member** with its own name, instantiated from this template like any other.
- **No archive.** The close pass writes into the memory, and the export writes a
  seed set; neither of them is a second store of conversations. `write` and
  `turn_write` still cross this level, and where a day's record belongs is the
  parent's decision, not this level's — but neither crosses it *untouched* any
  more: `write` also fires the close pass, and since #527 `turn_write` also
  writes the episode, because the level that holds the memory is the level that
  has to fill it.

## Versioning

`1.4.0` takes the **second** digit too, and for the plain reason: a caller can now
do two things that were never promised before. `pack_ack` leaves the level
(GH #458) — the receipt of an identity `./affinity` pushed into a generation, which
nothing in here consumes and nothing in here can — and `in_import` enters it
(GH #467), the return leg of `in_export` against a hive that is already running.

GH #527 lands in that same unreleased `1.4.0` and takes no digit of its own
either, and for a second reason on top of the wave rule below: the lane lists do
not move. `turn_write` was already declared, it is still declared, and it still
leaves the level — what changed is that a sibling inside now takes a copy of it.
A parent sees the same eleven exits across the release boundary; what it stops
seeing is one dead letter per stored turn at the root of its colony.

GH #471 lands in that same unreleased `1.4.0` and takes no digit of its own, for
the reason GH #454 and GH #459 shared one before it: a version is a shipped fact,
`1.4.0` has not shipped, and cutting a `1.5.0` for the second half of one
unreleased wave would invent a version nobody could ever have wired against.
Across the release boundary a parent sees one addition. What moved underneath is
real all the same — the lane fans out to three holders, the sink files by hive,
`export_done` travels three times with `hop.export_hive`, `hop.import_hive`
picks the holder on the way back, and `./affinity` gained a `reject` exit. Both
occupant pins moved with it, and the pinned versions live where they belong — in
`affinity/config.json` and `firewall/config.json`, not in this prose.
Nothing was taken away, so every parent wired at `1.3.0` is still wired
correctly; what it does *not* have is the two new lanes, and an undeclared lane
at a level boundary is a message that dies as `no_route`.

`1.3.0` took the **second** digit before it, and the reason there was the opposite
one: the lane lists did not move at all. Every name a parent wired at `1.2.0`
still meant the same thing.
What grew was what the level can *do*: two containers that were not there, ten
edges that were not there, and new producers on lanes that were already declared.
The `answer` lane is the clean case of that distinction — `org` and
`meclaw-os` both describe it as *a turn answered, a brief read*, and the
lane was declared at this level and at both of those long before now: until 1.3.0 only
the brief half could ever travel it, because the channel that consumed the answer
stood inside the generation.

**GH #454 and GH #459 land in the SAME `1.3.0`**, and that is a rule rather than
a shortcut: a version is a shipped fact, `1.3.0` has not shipped, and cutting a
`1.4.0` for the second half of one unreleased wave would invent a version nobody
could ever have wired against. The digit is decided by what a parent sees across
the release boundary, and across that boundary the two are one addition.

The assistant's move in the same wave is the contrast: it lost `./channels` and
the `turn` lane, and a removal takes the first digit (`docs/development-rules.md`
§ 4). The occupant pins in `affinity/config.json`, `memory-hive/config.json` and
`firewall/config.json` are version-pinned on purpose — a bare name resolves to
the highest version present, which is the drift `registry.template_chain` exists
to make visible.

Nothing in this template pins the `display` or the `colony-view` template. A
screen
and an app are **instantiated** into their containers, never shipped inside them,
and a `ref` on a template that did not travel refuses the mutation that carries
it — which is exactly the state a member with no screen yet is legitimately in.
