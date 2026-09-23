# `member@1.9.3`

One person, as a level. **Four holders, three open containers and no cell of
its own** — seven nodes and sixty-six edges.

| holder | what it holds |
|---|---|
| [`affinity`](../affinity/README.md) | **identity and meaning** — the curated record of who this person is and who their people are to them. Curated, fail-closed, quotable: it answers *who is X to me* and it is the only thing that answers it. |
| [`memory-hive`](../memory-hive/README.md) | **observations**, tagged with the participant set they were learned in. Raw, allowed to be wrong, carrying a confidence — this is what was said, not what it means. |
| [`firewall`](../firewall/README.md) | **the screen**. Every inbound turn is measured before it reaches anything of this person's, and the verdict is a comparison or a clock, never a model. |
| [`access`](../access/README.md) | **the keys** — since 1.5.0 (GH #560). The provider credentials this person's agents authenticate with, held by the person rather than by the OS. Nothing in this level's graph reaches it but the drain for its `error` lane; a brain asks it over a v-lane. |

Beside them stand `assistants` and — since 1.3.0 — `channels` and `apps`, three
real, empty, open containers. Since 1.6.0 there is nothing else: the one `code`
cell this level owned, `export-sink`, is gone with
[#555](https://github.com/mmeyerlein/meclaw/issues/555) — every holder's own
store writes its seed set now, so the level composes nothing and files
nothing.

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

Eight lanes in, twelve out — **plus six that are not at this rim at all**: `recall` and
`in_bundle` ([#562](https://github.com/mmeyerlein/meclaw/issues/562)) and, since 1.6.0,
`tool`, `in_tool`, `schemas` and `in_menu` ([#552](https://github.com/mmeyerlein/meclaw/issues/552))
carry `at: ["./assistants"]`. Both roads start inside `./assistants` and end inside
`./memory-hive`, siblings in here; the declaration is what makes this level a hop nothing
may skip, because this level is where the round a recall is asked in gets stamped. Every one
of the twenty-six is a lane an occupant actually has at the version pinned above; nothing
here describes a lane a holder lost.

### The memory road has two legs (1.6.0)

The AMBIENT one is older: `recall` up from an asker, `in_bundle` back down, one bundle per
turn before the model has read it. The DELIBERATE one is the tool
([#552](https://github.com/mmeyerlein/meclaw/issues/552)), and since `memory-hive@3.2.0` the
memory answers it itself:

| edge | lane | what it does |
|---|---|---|
| `./assistants -> ./memory-hive` | `tool`, `hop.tool_name == 'memory_recall'` | turns the call into the hive's own `tool_call` and stamps the round: `audience_now`, `channel`, `session_id`, `turn_id` |
| `./memory-hive -> ./assistants` | `tool_result` | the answer, restamped to `in_tool` — an ordinary tool result re-entering the round that asked, refusals included |
| `./assistants -> ./memory-hive` | `schemas` | a generation's menu tick, turned into the hive's own `in_schemas` |
| `./memory-hive -> ./assistants` | `tool_schemas` | the declaration, restamped to `in_menu` and stamped `context.tool_answerer = 'memory'` — the key the menu merge of #529 files an answerer under |

They are **template** edges rather than v-lanes, and that is the one place the two legs
differ: this level is a mandatory hop for both, so nothing is bought by drawing the tool
road per generation, and an edge on disk is an edge an audit can read.

| in | goes to | the caller promotes |
|---|---|---|
| `in_turn` | the screen | `context.channel` — the chat or room this turn is in — and `context.user_id` if this colony has per-user firewall rules. The door edge promotes what it finds and falls back to the empty string, so an unpromoted channel costs one shared rate bucket rather than a vanished turn. A caller that wants the answer routed back into one of this member's own channels promotes `context.channel_node` as well (§ *The two channel keys*); an operator that does not gets the answer out of the level, which is what it asked for |
| `in_recall` | the memory, as its own `in_query` | `hop.recall_query`, `hop.memory_tier`, `hop.recall_window_from`, `hop.recall_window_to`, plus the round: `context.audience_set` and `context.channel`. The lane carried a correlation id as well until 1.6.0; it does not any more ([#552](https://github.com/mmeyerlein/meclaw/issues/552)), because the hive's own `bundle` exit branches on exactly that key to tell a tool round from the ambient one, and a door that set it would send every outside question through the adapter. The answer comes back on `bundle` and the refusal on `reject`, both out of this level — see § *The asker outside* ([#533](https://github.com/mmeyerlein/meclaw/issues/533)); until then the lane promised an answer the level had no exit for |
| `in_brief` | the record, to read | `context.asker` and `context.audience_set` |
| `in_propose` | the record, to write | `context.actor`, and `context.subscriber` for a `subscribe` |
| `in_build_result` | `./assistants`, under the same name | nothing. Which generation it belongs to is decided by the per-instance edge inside the container, the same way `in_bundle` finds its way home |
| `in_export` | **all three holders**, unchanged — and `./assistants` as a fourth when the caller names a generation | nothing, or `context.assistant`. All three holders declare an empty context: an export is about the whole member, never about a round. The fourth target is the exception and it is an ADDRESS rather than a round: a member with two generations has two session ledgers, so the keeper is named, never fanned to ([#475](https://github.com/mmeyerlein/meclaw/issues/475)). Since 1.4.0 the lane fans out — until [#471](https://github.com/mmeyerlein/meclaw/issues/471) only the memory answered it |
| `in_import` | the holder `hop.import_hive` names, memory by default | nothing, or `context.assistant` when `hop.import_hive` ends in `/session-keeper` (`talky/session-keeper`, `talky-chat/session-keeper`) — that part has to reach one generation, and the container reads the same key a turn is addressed with. One export part per message, idempotent; the receipt rides the `dump` lane this level already drains (since 1.4.0, GH #467, GH #471 and GH #475) |

| out | from | what it is |
|---|---|---|
| `answer` | the record **or** an assistant | **two producers since 1.3.0.** From `./affinity` it is the brief, told apart from a push by `hop.subscriber == ''`. From `./assistants` it is a turn ANSWERED whose caller named no channel of this member — an operator, a digest, a second person's agent — carried out by a **guarded default**. An answer that *does* name a channel never reaches this lane: it is carried into `./channels` instead |
| `bundle` | the memory | the answer to a question that came in at this level's own `in_recall` door — the recalled material, on its way back to an asker OUTSIDE this member ([#533](https://github.com/mmeyerlein/meclaw/issues/533)). It leaves only for that asker: the exit is guarded on `hop.recall_caller == 'outside'`, and every other bundle goes DOWN into `./assistants` on the default edge, exactly as it always did |
| `ack` | the record | a proposed change was accepted or rejected, with its reason code |
| `reject` | the screen **or** the memory | a refusal. `hop.reject_reason` says which case. A refused RECALL is sorted like the answer to one since #533: the outside asker's leaves here, an assistant's goes back down as `in_bundle` |
| `error` | the record, a **channel**, an **app** **or** an assistant | a failure that was not a refusal. Which cell inside produced it is not the caller's business; the member is a boundary, not a consumer. The channel source arrived with #454 — a connector's own failure used to leave through the generation that held it. The app source arrived with #459, for the same reason, and so did one more case that is not an app's at all: a screen `event` or `receipt` whose owner this level cannot place leaves here, carrying the lane it was on in `hop.kind`, rather than dead-lettering where nobody would look for it |
| `write` | an assistant | a batched conversation write, on its way past this level. It is **also** fanned onto the memory's close pass and is not consumed by that fan-out |
| `turn_write` | an assistant | one finished turn, offered for archiving as it is produced. Like `write` it is **also** fanned down — onto the memory's `in_episode` lane since #527 — and is not consumed by that fan-out |
| `prune` | an assistant | a housekeeping report, raised when something above fired `in_prune` |
| `build` | an assistant | a structural wish or a submission leaving one of this person's generations, on its way to the one baumeister the colony shares. The member neither reads it nor answers it: everything between the tool surface and the OS level is transit (GH #425) |
| `close_report` | the memory | what one close pass did to an ended session: added, sharpened, corrected, closed, restated, and the three counts that say what it could not do |
| `export_done` | a holder, or a generation's keeper | that holder's seed set is complete on disk — every table written and `export_final.json` beside them, written last. `hop.seed_dir` says where, RELATIVE to the fence that holder's store declares, and `hop.export_hive` says which holder; three travel per export, four when a generation was named. Since 1.6.0 the holder says it ITSELF ([#555](https://github.com/mmeyerlein/meclaw/issues/555)); before that a cell of this level said it for all of them |
| `dump` | a holder, or a generation's keeper | the receipt of one applied import part: `hop.rows_written` counts the inserts it dispatched, so zero means the target already had every row. It is the only positive signal an import has, and since 1.6.0 it LEAVES the level — until then it ended inside the member, in the cell that existed for the export half and read a receipt without saying anything about it |
| `pack_ack` | an assistant | the receipt of one identity pack `./affinity` pushed into a generation. Nothing here consumes it, and nothing here can (since 1.4.0, GH #458) |

The `assistant` level emits **nine** lanes since `assistant@2.2.0`, and this
level places every one of them. The one lane it ACCEPTS that this member does not
carry, beside the four operator lanes, is `in_pack` (GH #458): its producer is
inside this level rather than above it — `<member>/affinity` is the record two
assistants of one person read — so the push edge goes from one sibling to another.
Since GH #561 it goes there as a **v-lane** and ends two storeys down, at
`<member>/assistants/<agent>/talky` and `<member>/assistants/<agent>/cogny`: the
generation declares those two rims as the connect points of the lane
(`"at": ["./talky", "./cogny"]`) and no longer carries the pack itself. A lane at the
member's own door would be an interface promising something nothing outside ever
sends.

| the assistant emits | what this level does with it |
|---|---|
| `answer` | into `./channels` when `context.channel_node` names one, **out** on `answer` when it does not |
| `recall` | consumed — into the memory as `in_query` |
| `sidecar` | consumed, and **sorted by section** (since 1.7.0, [#607](https://github.com/mmeyerlein/meclaw/issues/607)): `hop.section == 'memory'` into the memory as `in_remember`, everything else into `./apps`. `extraction` still carries the memory block on its own lane beside it |
| `write` | **both**: fanned onto the memory's `in_close_pass` *and* out on `write` |
| `turn_write` | **both** (since #527): fanned onto the memory's `in_episode` *and* out on `turn_write` |
| `prune`, `error`, `build` | out, untranslated. Nothing here consumes them |
| `pack_ack` | **out**, untranslated (GH #458). Nothing here consumes it: affinity's own record of a delivery is the `sent_at` it writes itself, and the hive has no lane that takes a receipt — so a receipt is evidence for whoever operates the colony. Two travel per pack, one per occupant of the generation. Since GH #561 each one leaves its RIM on a v-lane and stops at `./assistants`; this level's own `./assistants -> .` edge is what takes it the rest of the way out, exactly as it does for `write`, `prune` and `error` — a boundary exit rather than a hop of the identity chain, and it stands whether or not any generation subscribed |

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
several askers of its own tells its rounds apart on a hop key of its own, which
crosses the hive untouched; the door promotes no correlation at all since 1.6.0
([#411](https://github.com/mmeyerlein/meclaw/issues/411),
[#552](https://github.com/mmeyerlein/meclaw/issues/552)).

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
  crosses the generation.** Since 1.9.0 it promotes one key more, `engine`, out
  of `hop.engine`: the firewall deletes its own `fw_*` keys on the way out and
  nothing else, so the key reaches the generation on the same message.

**The delegation, which is not a turn.** `./channels -> ./assistants` carries
`delegation` straight across, re-stamped to `in_delegation`, with `channel`,
`engine` and `delegation_id` promoted onto context (1.9.0). It is the one lane
out of a channel that does NOT go through the firewall, and the reason is the
firewall's exit rather than the firewall itself: that exit stamps `in_turn`, and
a handover that arrived as a turn would be answered as one. What screening it
needs it already had, as the turn the delegation came out of.

**The advice, going back down.** `./assistants -> ./channels` carries the three
`sidecar` sections a front model may append for a channel — `fact`, `context`,
`correction` — re-stamped to `in_advise`, guarded on `context.channel_node`
(1.9.0). The `sidecar` edge into `./apps` is unchanged and still fires on the
same message: an app that offered one of those sections keeps getting it, and
the channel gets it too.

**Both of those edges end at a CONTAINER, and a container is not a
pass-through.** `Edge.to` is a static path here as everywhere, so each lane needs
one more edge — `./assistants -> ./assistants/<generation>` on `in_delegation`,
`./channels -> ./channels/<channel>` on `in_advise` — drawn by the mutation that
grew the child, because only that mutation knows its name. Since `builder@1.12.0`
the recipe renders both from its table ([#803](https://github.com/mmeyerlein/meclaw/issues/803));
a generation or a channel grown before it is missing its last leg, and every
delegation and every advice section stops at the container as `hive_no_route`.

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
(§ *The asker outside*). The write half is **one** edge, and
it writes the facts:

- `sidecar` **with `hop.section == 'memory'`** → `in_remember` writes the
  **facts** the front model annotated inside its own answer. It is the second
  half of the recipe `talky` prescribes to its parent (*two edges, never one*,
  [`../talky/README.md`](../talky/README.md) § the sidecar); the first half is
  inside the talky, and the drain the recipe asks for is the `reject` lane above.
  Since 1.7.0 the front model appends ONE fence with one key per section instead
  of one fence per obligation, and the splitter inside the generation cuts it
  into one message per section
  ([#607](https://github.com/mmeyerlein/meclaw/issues/607)). The memory section
  is the annotation this level has always written, so this is the `extraction`
  edge with a different condition — same target lane, same three promoted keys,
  same hive. The two-phase rebuild lives in the SHAPE rather than in the lane:
  the ingress at the far end reads the section's `payload` and the old
  block-in-a-turn alike, without being told which colony it is standing in.
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
level promoted seven of them and not that one. It never showed on the tool path of the day,
which is why it shipped: a `memory_recall` call happened *after* the brain call and the brain
edge has promoted `hop.turn_id` into context long before. The AMBIENT leg leaves before the
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

**The export, and the cell this level no longer owns.** `in_export` crosses the
boundary untouched onto the `in_export` of **all three holders at once** —
`./memory-hive`, `./affinity` and `./firewall` — and, when the caller names a
generation, onto a **fourth** target: `./assistants`, through which the demand
reaches that generation's own session keeper four levels down
([#475](https://github.com/mmeyerlein/meclaw/issues/475)). What each of them
does with it changed completely in 1.6.0
([#555](https://github.com/mmeyerlein/meclaw/issues/555)), and the ruling behind
it is one sentence: *cells manage their own files, nobody else does.*

Every holder's own STORE writes its seed set, through the substrate's `transfer`
slot: `<fence>/<hive>/seed/<table>.jsonl`, the schema declaration as line 1 and
one row per line after it, which is the birth format
[`../memory-hive/README.md`](../memory-hive/README.md) § *The document*
specifies and the two other holders' stores read the same way. The fence is a
`params` declaration of that store — `params.transfer.base_path`, absolute, the
precedent being `file`'s `base_path` — and `hop.export_to`, which the operator's
own trigger passes through, names the directory of ONE run under it. Each
holder's porter names the directory (its own template name), asks its store to
write, and raises `export_done` off the receipt the slot hands back:
`hop.export_hive`, `hop.seed_dir` (relative to that fence) and
`hop.rows_written`. **Three travel per export**, in whatever order the walks
finish in — **four** when a generation was named and its keeper wrote its ledger
out beside them.

**What this level lost, and it is worth stating rather than implying.** Until
1.5.1 the parts travelled as MESSAGES on `dump`, and one `code` cell of this
level, `export-sink`, turned the stack back into the files it had been all
along. That cell also wrote a **member-level** `export_final.json` beside the
per-hive directories, naming every holder that had finished. **That document no
longer exists.** It was a composition statement about four hives, written by the
one thing that saw all four, and the substrate knows no hives at all — it writes
the marker of the CELL that wrote the directory. What replaces it is that every
directory says for itself whether it is whole: `<hive>/seed/export_final.json`
is written last and by one rename, so a reader that watches it never meets a
directory that is still filling.
[`../../examples/memory-import/build_import.py`](../../examples/memory-import/build_import.py)
reads exactly that, one marker per directory, and refuses a directory without
one.

**The fourth target is guarded, and the guard is the point.** The edge reads
`hop.route == 'in_export' && has(context.assistant) && context.assistant != ''`.
Two measurable reasons, neither of them taste. A member with two generations
holds **two** session ledgers and they are not one document -- a keeper files its
document under its own path inside the generation (`<fence>/talky/session-keeper/`,
`<fence>/talky-chat/session-keeper/`, since `session-keeper@2.2.2`,
[#712](https://github.com/mmeyerlein/meclaw/issues/712)), so two GENERATIONS walked by
one export would write the same two directories and each would hold whichever walk
finished last, silently. The keepers of the ONE generation that is named are all
walked, and each says `export_done` for itself. An export that names no generation is therefore exactly the
export this level always did — three holders, no keeper, and no dead letter,
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

Three details there are decisions rather than defaults. **One directory per
hive** is a requirement, not tidiness: `memory-hive` and `affinity` both declare
a table called `entities`, so a flat directory would have written one over the
other without a word. Each holder's porter names its own hive and the store
writes under that name. **The `export_done` edges test `hop.route` and nothing
else**: every one of the holders pairs `in_export` with `export_done` in
`params.required_drains`, and the probe that checks the pairing runs the
described hop through the real edge evaluator, so an edge additionally guarded
on a second key evaluates false under it and reads as no drain at all — one
plain edge per holder. **And the fence is never canonicalised at boot**: a
member whose export directory does not exist yet still boots and still passes
`--validate`, and finds out at the first `to`/`from` with a `transfer_io_error`.
That is the same reason the sink was a `code` cell rather than a `file` cell,
kept as a property of the substrate instead of as a property of a cell nobody
else could see.

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
are, because `in_import` is the name on both sides of this boundary. The receipt
of an applied part rides `dump`, and since 1.6.0 that lane LEAVES the level
(#555): until then it ended in the sink, which read it and said nothing about
it — the one arrangement [#284](https://github.com/mmeyerlein/meclaw/issues/284)
forbids, and it only ever ended there because that cell existed for the export
half.

**Which holder a part belongs to is edge truth.** A body is model-writable and
an edge is not, so the holder is read off `hop.import_hive`: `'affinity'` and
`'firewall'` each have their own guarded edge, and `./memory-hive` is the
**guarded default** ([#283](https://github.com/mmeyerlein/meclaw/issues/283)) —
the router evaluates it only when no guarded edge decided. That is not a
courtesy to lazy callers; it is what keeps every part written before
[#471](https://github.com/mmeyerlein/meclaw/issues/471) arriving where it always
arrived, since the memory hive was the only place it could have come from.
A keeper is the third guarded address and it is the odd one out. `hop.import_hive`
means the same thing for every holder -- the directory under the export root the part
came out of -- and for `affinity` and `firewall` that is the hive's name, while a
keeper's is its path inside the generation, `<talky>/session-keeper` (since the
keeper's 2.2.2, [#712](https://github.com/mmeyerlein/meclaw/issues/712)), so its
edge reads `hop.import_hive.endsWith('/session-keeper')`. That edge goes to `./assistants`, because the hive it names stands four levels below the
deepest endpoint this level can address ([#475](https://github.com/mmeyerlein/meclaw/issues/475)).
Which generation's keeper a part lands in is then the container's own question,
answered on `context.assistant` — and a part that names the keeper and no
generation has no address at all, so it stops as `no_route` at
`<member>/assistants` rather than being handed to a holder that would refuse it
under some other name. **Four** doors, exactly one of which can fire per
message. A part addressed the way it was before the keeper's 2.2.2, with a bare
`session-keeper`, does not end in `/session-keeper`: the guarded default takes it
and it reaches the memory hive, not a keeper. `examples/memory-import/build_import.py`
reads that old form as `talky/session-keeper`, so an old export sent through it
arrives; a part sent by hand names the keeper's path.

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
`in_episode`, exactly the way `write` is fanned onto the close pass. An
assistant's `export_done` and `dump` join them since 1.6.0: the keeper of a
named generation writes its own ledger and says so, and the receipt of an
applied import part is a receipt like any other — both leave on one plain edge
each, where until then the level consumed them (#475, #555). This level owns no archive and no timer, so it has
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

**A `channel` value may be composed, and a peer channel is where that starts to matter.**
Every channel until now had one counterpart -- a Telegram chat, a phone line, one screen --
so the node name was enough to tell two conversations apart. A channel that speaks to other
colonies has many, and one value per channel would put every counterpart into one generation,
one idle clock and one rate bucket. The ingress edge of such a channel therefore writes the
class and the counterpart together, `'peer-friend:' + string(hop.peer)`, with `has(hop.peer)`
in its `condition` for the reason the two keys above are written `has(...) ? ... : ''`: a
modifier that cannot evaluate skips the edge, and a turn that vanishes is worse than a turn
that is refused. Nothing downstream learns a new word. `session-keeper` reads the value as the
opaque string it always was and opens one generation per counterpart, `firewall` keeps one
bucket per counterpart, `memory-hive` writes the conversation down as its own room -- and the
answer still finds its way back, because the way back hangs on `channel_node`, which stays the
node.

**Since 1.9.0 there is a third key beside them, and it is not an address at
all: `context.engine`.** A channel whose model answers on its own timeline —
one live provider that hears and speaks, instead of a recogniser, a model and a
synthesiser in a row — stamps `hop.engine` on what it sends up, and this level
promotes it onto context on the edge into the firewall and on the delegation
edge. It is promoted rather than read once for the reason `channel` is: a hop
survives one delivery, and whatever finally answers stands several hops away.
What reads it is the assistant's own collector, which asks a live session for
ADVICE and a written channel for a reply — the same question, answered in two
shapes. A channel that stamps nothing leaves the key empty, and empty is the
value every reader treats as the ordinary kind, so no colony has to be rewired
for a key it does not use.

**For a chat channel: two lanes up, one lane down, and no more.** (A screen is a
channel too and carries more — see *The display channel* below; a channel with a
live model carries two more again, and they are the last two rows here.)

| direction | lane | who ships the edge |
|---|---|---|
| up, from the channel | `turn` — a raw inbound message | the channel's mutation draws `./channels/<name> -> ./channels`; **this level** ships `./channels -> ./firewall`, which re-stamps it to `in_turn` and promotes `context.engine` |
| up, from the channel | `error` — the connector's own failure | the channel's mutation draws it; **this level** ships `./channels -> .` |
| down, to the channel | `answer` — what an assistant said | **this level** ships `./assistants -> ./channels`; the channel's mutation draws `./channels -> ./channels/<name>`, guarded on `context.channel_node == '<name>'` |
| up, from the channel | `delegation` — a live model handed work back (1.9.0) | the channel's mutation draws it up with the rest; **this level** ships `./channels -> ./assistants` DIRECTLY, re-stamped to `in_delegation`; the generation's mutation draws the last leg into the child, guarded on its own name |
| down, to the channel | `sidecar`, sections `fact`, `context`, `correction` (1.9.0) | **this level** ships `./assistants -> ./channels`, re-stamped to `in_advise`; the channel's mutation draws the last leg, guarded on its own node name |

**The delegation edge goes round the firewall on purpose.** The firewall's exit
stamps `in_turn`, so a handover that went through it would arrive as a turn —
and a generation ANSWERS a turn, which is exactly what a delegation is not. It is
not a screening hole either: the audio it belongs to was screened as the turn it
came out of, and the body carries the caller's own words rather than anything
new.

**The advice edge fans out rather than switching.** The `sidecar` edge into
`./apps` is untouched, so a section an installed app offered still reaches that
app; the new edge carries the same three sections to the channel as well. One
producer, two consumers, and neither has to know about the other.

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

**Adding a channel costs one node and four edges since `member@1.9.0`, and no template moves.**

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
     "condition": "has(hop.route) && hop.route == 'answer' && has(context.channel_node) && context.channel_node == 'telegram'"},
    {"from": "./channels", "to": "./channels/telegram",
     "condition": "has(hop.route) && hop.route == 'in_advise' && has(context.channel_node) && context.channel_node == 'telegram'"}
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

Since GH #459 the cell that stands there is real: [`display@2.7.0`](../display/).
**Three** edges instantiate one — one fewer than a chat channel since
`member@1.9.0`, because nothing advises a display, though two of
them point down where a chat channel's point up — and the second says the only
thing a chat channel's edges do not, the third the one thing a chat channel
never hears:

| edge | condition | why |
|---|---|---|
| `./channels/display-<s> -> ./channels` | `event` or `receipt` | what the screen produced, stamped with `context.channel_node` and `context.channel`, which on a screen are the same word |
| `./channels -> ./channels/display-<s>` | `view` or `withdraw`, `context.channel_node == '<s>'` | re-stamped with ONE ternary to the display's own `in_view`, or to `in_withdraw` for a view that is over (`member@1.9.3` carries the lane out of `./apps`; [`builder`](../builder/README.md) renders this edge) |
| `./channels -> ./channels/display-<s>` | `error` | a channel's failure, re-stamped to the display's `in_notice` — since `builder@1.10.0`, drawn by the mutation that grows the screen |

**A view comes down the way it went up.** Since `member@1.8.0` the edge that carries
what an app drew out of `./apps` carries `withdraw` beside `view`, in the same edge
rather than in a twin: the two are one lane pair of one producer. `display` has accepted
`in_withdraw` since it shipped — *a view outlives the turn that produced it, so ending
one has to be a message, and a `ttl_ms` is the timer version of the same wish rather than
a replacement for it* — and until 1.8.0 nothing carried the asking. It became
load-bearing with several views per app: a timer that has rung and a card that has been
replaced have to say they are over instead of standing there fading.

**This level carries only the first half, and that is not an omission.** `./apps ->
./channels` is the member's own edge and it is what moved. The down-edge onto a screen is
the INSTANTIATING MUTATION's, exactly as its `view` twin always was: `Edge.to` is a
static path in this substrate, and a template cannot name a screen it has not met.
Since `builder@1.11.0` its `grow_level` recipe renders that edge with the withdrawal in
it, and `examples/organism/grow-screen.json` is its byte truth.
**A screen grown before `builder@1.11.0` needs that one edge redrawn**, or a withdrawal
reaches `./channels` and stops: `hive_no_route` in the DLQ, once per withdrawal (measured
in `crates/meclaw-cells/tests/709_an_app_takes_its_view_down.rs`). What the edge looks like is in the
table above — one edge, one ternary, never two.

**A screen has no error wire, and drawing one would be drawing into the void.**
A connector's third edge exists because a connector *emits a failure of its own*:
`telegram-connector` declares an `error` lane and stamps `hop.error_code` on it,
which is what the `has(hop.error_code)` edge above catches. The display declares
exactly two emissions — `event` and `receipt`
(`templates/display/config.json`, `params.contract.emits`) — and its graph
carries exactly the two matching edges out of the hive. A refused write **is**
the display's failure lane, and its `error_code` travels in the **body** of the
receipt beside `owner`, `view_id` and a detail string, while the hop carries
`route`, `owner` and `view_id` and never an `error_code`
(`templates/display/compose/compose.py`, `refuse`). So a third edge on
`has(hop.error_code)` would have no producer: nothing a screen puts on the wire
could ever match it, and a receipt — the one thing that *does* carry the word —
would not, because it carries it a compartment away. The substrate's own
`contract_violation` reply is addressed to the caller's `reply_to` and never
routed by this graph, so it owes no edge either.

**A screen has an error wire *in* since `builder@1.10.0`:** a channel's failure
inside the container is re-stamped `in_notice` towards the screen, so the person
sees it as a system notice; the exit edge stays, so the operator sees it too.
The wire is the other direction from the one the paragraph above refuses: it
carries the failures of the *other* channels — a microphone that caught nothing,
a voice that could not speak — down onto the screen, never anything the screen
produced. It has no `channel_node` guard, because an `error` carries no context
of a screen and belongs to the member, and it cannot loop, because the screen
emits no `error`. It is drawn by the **builder's** recipe rather than shipped by
this level, for the reason the whole `channels` container carries no edge onto a
child: this template does not know the screen's name, and `Edge.to` is a static
path. `examples/organism/grow-screen.json` is the byte truth of the three edges;
the sentence the screen makes of a code is the display's
([`display`](../display/) § *A channel's failure is one of them*, ADR-0039).

**The edge down is for a producer of view bodies, and an agent's answer is not
one.** A view is a body carrying `view_id`, `kind` and `content`. A talking
agent's `answer` carries `messages[]` and nothing else, and the screen refuses it
by name: `invalid_view`, with the reason `"view_id" must match [a-z0-9-]{1,64}`
(`templates/display/compose/compose.py`, `validate`). The claim that stood here
until [#597](https://github.com/mmeyerlein/meclaw/issues/597) — *the smallest view
needs no app* — read the smallest KIND of view (prose, which needs no component
tree) as the smallest WRITER of one. Prose is the cheapest view to write;
somebody still has to write it.

So the smallest screen shows nothing of the conversation, and that is a
legitimate state — the ordinary one for a member that has only just grown a
screen. A member that wants an agent's prose on its screen installs a producer of
views beside the agent: an app at the rim of the member (`./apps`, see § *What
transits `./apps`*), or any cell built to emit `view`. Whoever writes the body
writes the three keys.

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

| lane | owner path contains | goes to | as |
|---|---|---|---|
| `event` | `/assistants/` | `./assistants` | `in_turn`, with `hop.kind` set to `event` |
| `event` / `receipt` | `/apps/` | `./apps` | lane name kept |
| `event` | neither, or empty | `.` | `error`, with the original lane on `hop.kind` |
| `receipt` | anything but `/apps/` | `.` | `error`, with the original lane on `hop.kind` |

Two things about that table are deliberate.

**It matches with `contains`, not with a prefix.** The owner is an *absolute*
cell path and a template does not know its own absolute prefix. `contains('/assistants/')`
is the prefix test a level-relative template can actually write.

**An agent gets an EVENT as `in_turn`.** The `assistant` level accepts no event
lane and did not grow one for this: `in_turn` is the lane it has, and `hop.kind`
is what tells a brain that this turn came from a button rather than from a
keyboard. `context.channel_node` travels with it, so the answer finds its way
back to the same screen.

**An agent gets a RECEIPT not at all** — since GH #598. A receipt is feedback to
a WRITER, not something a person said, and a generation reads a turn by answering
it: the answer went back to the screen, the screen refused it again, and the
level had built a loop that cost one brain call per round (44 in six minutes on
one live colony, 68 on another). The level treats it the way it already treats
`pack_ack`: *the hive has no lane that takes a receipt*, so the receipt is
evidence for whoever operates the colony and leaves on `error` with the original
lane on `hop.kind`. An APP keeps its receipts — an app declares the lane, is the
producer of the view that was refused, and does not answer a refusal by writing
prose. See `plans/adr/0025-a-receipt-is-feedback-to-a-writer.md`.

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
keeper pairs `in_export` with `export_done` and `in_import` with `dump` in
`params.required_drains`, and the probe that checks a pairing runs the described
hop through the real edge evaluator — so an edge that additionally tested a
second hop key would evaluate false under it and read as no drain at all. What
arrives on them is the keeper's own completion word and the receipt of an
applied part, and since 1.6.0 this level carries both OUT rather than filing
them (#555).

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
  "add_nodes": [{"name": "alex", "template": "member@1.9.3"}]
}}
```

then one mutation per assistant — the addressing edge plus the fifteen transit
lanes (`../assistant/README.md` § *Instantiating* writes them out):

```json
{"scope": "<member>", "diff": {
  "add_nodes": [{"name": "assistants/scribe", "template": "assistant@2.8.1"}],
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

## The credential v-lanes

Since `member@1.5.0` the level carries an **`access` of its own** — the shipped
broker, as an occupant beside the memory and the record (GH #560). The pin lives
in `access/config.json`, where a ref marker's pin belongs.
Authorisation and key ownership are two different jobs that used to share one
hive: the shell's `access` answers the submitter's policy questions and keeps
doing that, but the provider keys a person's brains burn belong to the **person**.
That is the same argument that puts the memory hive here (ADR-0013), and it is
why the broker stands at this level rather than one up.

Nothing in this template's graph reaches it except one edge:

```json
{"from": "./access", "to": ".", "condition": "has(hop.route) && hop.route == 'error'"}
```

the drain the broker's own README says its parent **must** wire. Everything else
about the credential lane is drawn by the manifest that grows a generation,
because a lane has two ends and the second one does not exist until then.

### The form

Per generation and per surface — `talky` and `cogny` — **two edges**, both
carrying a `lane` (GH #559). They are drawn at the **member's own scope**,
because an edge lives in the graph of the lowest common ancestor of its two
endpoints, and that is this level:

```json
{"scope": "/os/orgs/acme/members/alex",
 "diff": {
   "add_edges": [
     {"from": "./assistants/scribe/talky/brain",
      "to": "./access",
      "lane": "credential_request",
      "condition": "has(hop.route) && hop.route == 'credential_request'",
      "modifier": {"set_hop": {"route": "'in_invoke'"},
                   "set_context": {"requester": "'agent:scribe/talky'"}}},
     {"from": "./access",
      "to": "./assistants/scribe/talky/brain",
      "lane": "in_sealed",
      "condition": "has(hop.route) && hop.route == 'ack' && has(hop.operation) && hop.operation == 'vault.deliver' && has(hop.grant_id) && hop.grant_id == 'grant:example-provider-primary@member-alex/talky'",
      "modifier": {"set_hop": {"route": "'in_sealed'"}}}
   ]}}
```

and the same pair again with `cogny` in place of `talky`. The runnable version of
this declaration is `examples/organism/grow-credentials.json`, which is the sixth
entry of `examples/organism/grow.manifest.json`; the whole round is measured in
`crates/meclaw-cells/tests/gh560_a_members_brain_gets_its_sealed_key.rs`.

Four things about that shape are load-bearing.

**It is a v-lane, and the target is what permits it.** Between the brain and the
broker lie three levels — `./assistants`, the generation, and `talky` — and the
innermost is **sealed** (`params.ports: []`). The edge lands on a cell inside it
anyway, and it is not a bypass: `talky` and `cogny` declare both
halves of this lane in their own contract with `"at": ["./brain"]`, which is the
one opening a template pronounces about **itself**. Take the `at` away and the
mutation is refused by name with `v_lane_no_connect_point`, and the refusal says
which string to add. The two levels in between declare nothing about the lane and
are therefore transparent — that is the sanctioned exception to the union rule
(`docs/development-rules.md` § 8b).

**The requester comes from the edge.** `set_context.requester` is what the broker
issues the grant to (R-AC-1); a body claiming a requester changes nothing. One
`requester` per consumer, and therefore **one grant per consumer**: the answer
edge is addressed by `hop.grant_id`, and two brains sharing one handle would both
be handed every sealed box — a box a cell never asked for costs the *other*
cell's parked turns their receipt.

**The lane is an ASK, never a subscription.** The brain mints an ephemeral X25519
recipient key per request and the box is sealed against it; forward secrecy is
exactly that fact. Nothing pushes a credential, and a woken brain asks again —
the value lives in one task's RAM and is written nowhere (`docs/cell-types.md`
§ *Sealed credential delivery*).

**A v-lane changes nothing about the secret.** The same ciphertext rides one hop
instead of four. What is journalled in `message_log` is the box — `epk`, `nonce`,
`ciphertext` — and the plaintext exists in no message at all.

### The two gestures that are not topology

A manifest cannot mint a grant into a store it has not grown, and it cannot put a
secret anywhere. So two acts stay with the operator, in this order:

1. **The vault's passphrase**, at birth, because there is no params-update
   operation: the `add_nodes` that grows this member carries
   `"override_params": {"access/vault": {"unlock_env": "<NAME OF A VARIABLE>"}}`.
   A vault inside a sealed hive cannot be unlocked over the user channel — it
   opens itself from the environment or not at all, and the param names a
   **variable**, never a value.
2. **The credential itself**, from stdin, with no colony running:
   `meclaw --root <root> --vault /main/…/members/alex/access/vault --vault-add cred:example-provider:primary`.
   A credential never becomes a message. `examples/vault-pilot/README.md`
   § *Running it* is the whole gesture in order, and
   `templates/access/README.md` § *A member's own broker* is the member-shaped
   version of it.

The grants themselves **are** topology-adjacent and travel in the manifest, as
`seed_rows` into `./access/store` — the door that writes them keeps a
`mutation_log` row, which is what a permission row deserves. One caveat worth
knowing: `seed_rows` creates the store's `cell.db` if the store has never woken,
and a store seed (`seed/<table>.jsonl`) only lands on a **fresh** database. A
member's `access/store` that took its grants through the door therefore does not
carry the seven shipped `policy` rows — which for a member's broker is the right
outcome and not a loss: those rows are about `colony.mutate`, and a person's
broker has no business granting that. Seed the `policy` row you *do* want
(the one that lets an expired grant be minted again) in the same declaration.

Finally, the brains have to name their grant — and **give up the key they
have**. `credential_grant_id` is **immutable**, and so is `api_key`, so both are
set where the generation is grown and neither can be repaired by a message
afterwards:

```json
"override_params": {
  "talky/brain": {"api_key": "",
                  "credential_grant_id": "grant:example-provider-primary@member-alex/talky"},
  "cogny/brain": {"api_key": "",
                  "credential_grant_id": "grant:example-provider-primary@member-alex/cogny"}
}
```

**The empty `api_key` is not tidiness, it is the switch.** A brain asks for a
credential only while it holds none, and the key in its config counts as one: set
a grant beside a non-empty `api_key` and the cell keeps spending the environment
key, never asks, and the four v-lanes carry nothing — quietly, because a model
that answers looks exactly like a model that answers. Both brain templates ship
`api_key: "${OPENROUTER_API_KEY}"`, so a generation grown without this line is a
generation whose credential lane is inert, and `api_key` being immutable means
the repair is a new generation rather than a message.

Both templates ship `credential_grant_id` resolving to the empty string, and an
empty string is no grant (GH #271): a generation nobody wires this way behaves
exactly as it did before and spends its `api_key`.

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
  `sidecar`, `recall`, `prune`, `error`, `build` and — since #475 — `dump`,
  the only one of them this level consumes rather than re-emits.

**What transits `./channels`** — `turn`, `error`, `event` and `receipt` up;
`answer` and `view` down. The container reads nothing in any of them: it carries
the message, and the address rule that decided where it goes lives in the edge
below it or in the owner the screen stamped.

**What transits `./apps`** — since 1.6.2 an app has three ways to be connected
at the rim of its member, and each one is a class of edge with an owner
(rulings 2026-09-04 and 2026-09-05):

| what the app does | how | who draws the edge |
|---|---|---|
| **listens** | a regular fan-out into the container: `turn` (the screened turn of a channel conversation), `answer` (what an assistant replied on that channel) and `partial` (an interim transcript of a voice channel), all three carried on down to the app by the binding edge | the mutation that installs the app |
| **offers** | a v-lane per direction *in*: the tool call and the menu tick end on the connect points the app declares for itself. The answers come back the ordinary way — `tool_result` and `tool_schemas` up into the container, restamped to `in_tool` and `in_menu` by the level's own two edges | the mutation for the v-lanes, the **level** for the two restamp edges |
| **writes** | `view` and `error` up, exactly as since GH #459, carried on by `./apps -> ./channels` | the mutation for the app's own outbound edge, the level for the rest |
| **is written to** | a section of the block the front model appends to its answer, since 1.7.0 ([#607](https://github.com/mmeyerlein/meclaw/issues/607)). No round, no call: the model writes the section into the same fence it writes the memory into, and it arrives as an ordinary message on `sidecar` carrying `hop.section` | **the level** for `./assistants -> ./apps`, the mutation for `./apps -> ./apps/<app>` on the section name |

`event` and `receipt` still come down from a screen the way they did, addressed
by the owner the display stamped.

**The rim does not know the sections, and cannot.** A section is named by
whoever OFFERED it — an app declares `display`, a second app declares something
else, and the offer is answered at menu time, long after this template was
written. So the level draws ONE edge, `./assistants -> ./apps` on
`hop.route == 'sidecar' && hop.section != 'memory'`, and the mutation that
installs an app draws `./apps -> ./apps/<app>` on the section that app answers
for. The split falls exactly where knowledge does: the level knows there is one
section it keeps for itself (`memory`, which goes to the memory hive on the edge
beside it, GH #122), the installer knows the app's name and the section it
offered, and neither has to learn the other's half.

That makes this edge the **second** exception to *whoever listens orders it*,
and for the same measurable reason as the two restamp edges: a section is only
ever written because it was offered, so on a member with no app the model is
never asked for anything but `memory` and nothing arrives on the edge. An
observer fan-out is different in kind — `turn` and `partial` exist whether or not
anybody listens, which is why those stay the installing mutation's.

**The level declares, the installing mutation draws** — and that split is the
whole of the 2026-09-05 ruling. This template names `turn`, `partial` and — since 1.7.0 —
`sidecar` as emits and `tool_result` and `tool_schemas` as accepts, all five with
`at: ["./apps"]`, which is the same sentence `recall` and `tool` already say
about `./assistants`: the lane is no lane of this rim, both its ends are inside
this level, and nothing may carry it PAST the member as a v-lane (ADR-0020). It
draws **no** observer edge of its own. *Whoever listens orders it* — the rule a
voice channel already follows with `emit_partials` — so a member with no
listening app carries no edge into an empty container and dead-letters nothing.
The two restamp edges out of the container are the first exception, and for a
measurable reason: without an installed app nothing ever arrives on them. The
`sidecar` edge into the container is the second, on the same measurement and for
the reason above it: the rim cannot name a section, and a section nobody offered
is never written.

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

### Installing an app

One mutation, scope `<member>`, and the builder renders it: the fast-lane recipe
`install_app` ([`builder`](../builder/) § *An app is a declaration*,
[#599](https://github.com/mmeyerlein/meclaw/issues/599)) draws the wiring from
the block the app's `template.json` carries under `app`. The wish hands that
block over verbatim — the recipe reads nothing but the wish. `<gen>` is the
generation the app offers its tool to, `<app>` is the instance name — which is
the template name, because an instance is named after its template — and
`<screen>` is the screen node in `./channels` the app draws on. This example
declares every kind at once: a screen, three listened lanes, a tool and a
sidecar section offered at `./show`, observed tool results at `./stage`, and a
device `<device>` it opens pages on.

```json
{
  "request": "install the app <app> into <member>",
  "recipe": "install_app",
  "params": {
    "scope": "<member>",
    "app": "<app>",
    "template": "<app>@<version>",
    "screen": "<screen>",
    "generation": "<gen>",
    "declaration": {
      "screen": {
        "out": [
          "view",
          "withdraw"
        ],
        "back": [
          "event",
          "receipt"
        ]
      },
      "listens": [
        "turn",
        "answer",
        "partial"
      ],
      "offers": [
        {
          "kind": "tool",
          "at": "./show",
          "tools": [
            "show"
          ]
        },
        {
          "kind": "sidecar",
          "at": "./show",
          "section": "<section>"
        }
      ],
      "observes_tool_results": "./stage",
      "drives": [
        {
          "cell": "<device>",
          "out": [
            "open"
          ],
          "back": [
            "page"
          ]
        }
      ]
    }
  }
}
```

It renders this declaration, and a test holds the two blocks together
(`crates/meclaw-cells/tests/gh599_an_app_is_installed_from_what_it_declares.rs`):

```json
{"scope": "<member>", "ctx": {}, "diff": {
  "add_nodes": [{"name": "apps/<app>", "template": "<app>@<version>"}],
  "add_edges": [
    {"from": "./firewall", "to": "./apps", "condition": "has(hop.route) && hop.route == 'pass'", "modifier": {"set_hop": {"route": "'turn'"}, "delete_context": ["fw_body", "fw_now", "fw_phase", "store_origin"]}},
    {"from": "./assistants", "to": "./apps", "condition": "has(hop.route) && hop.route == 'answer'"},
    {"from": "./assistants", "to": ".", "condition": "has(hop.route) && hop.route == 'answer' && (!has(context.channel_node) || context.channel_node == '')", "modifier": {"delete_context": ["tool_answerer"]}},
    {"from": "./channels", "to": "./apps", "condition": "has(hop.route) && hop.route == 'partial'"},
    {"from": "./apps", "to": "./apps/<app>", "condition": "has(hop.route) && (hop.route == 'turn' || hop.route == 'answer' || hop.route == 'partial')"},
    {"from": "./apps", "to": "./apps/<app>", "condition": "has(hop.route) && hop.route == 'sidecar' && has(hop.section) && hop.section == '<section>'"},
    {"from": "./apps", "to": "./apps/<app>", "condition": "has(hop.route) && (hop.route == 'event' || hop.route == 'receipt') && has(hop.owner) && hop.owner.contains('/apps/<app>/')"},
    {"from": "./apps/<app>", "to": "./apps", "condition": "has(hop.route) && (hop.route == 'view' || hop.route == 'withdraw' || hop.route == 'error')", "modifier": {"set_context": {"channel_node": "'<screen>'", "channel": "'<screen>'"}}},
    {"from": "./apps/<app>", "to": "./apps", "condition": "has(hop.route) && (hop.route == 'tool_result' || hop.route == 'tool_schemas')", "modifier": {"set_context": {"tool_answerer": "'<app>'"}}},
    {"from": "./assistants/<gen>/talky", "to": "./apps/<app>/show", "lane": "tool", "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'show'", "modifier": {"set_context": {"tool_caller": "'talky'", "assistant": "'<gen>'"}, "delete_context": ["col_phase", "consult_class", "consult_id", "tool_answerer"]}},
    {"from": "./assistants/<gen>/talky", "to": "./apps/<app>/show", "lane": "schemas", "condition": "has(hop.route) && hop.route == 'schemas'", "modifier": {"set_context": {"tool_caller": "'talky'", "assistant": "'<gen>'"}, "delete_context": ["col_phase", "consult_class", "consult_id", "tool_answerer"]}},
    {"from": "./assistants/<gen>/talky-chat", "to": "./apps/<app>/show", "lane": "tool", "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'show'", "modifier": {"set_context": {"tool_caller": "'talky-chat'", "assistant": "'<gen>'"}, "delete_context": ["col_phase", "consult_class", "consult_id", "tool_answerer"]}},
    {"from": "./assistants/<gen>/talky-chat", "to": "./apps/<app>/show", "lane": "schemas", "condition": "has(hop.route) && hop.route == 'schemas'", "modifier": {"set_context": {"tool_caller": "'talky-chat'", "assistant": "'<gen>'"}, "delete_context": ["col_phase", "consult_class", "consult_id", "tool_answerer"]}},
    {"from": "./assistants/<gen>/tools", "to": "./apps/<app>/stage", "lane": "tool_result", "condition": "has(hop.route) && hop.route == 'tool_result'"},
    {"from": "./memory-hive", "to": "./apps/<app>/stage", "lane": "tool_result", "condition": "has(hop.route) && hop.route == 'tool_result'"},
    {"from": "./apps/<app>", "to": "./<device>", "condition": "has(hop.route) && hop.route == 'open'", "modifier": {"set_hop": {"route": "'in_open'"}}},
    {"from": "./<device>", "to": "./apps/<app>", "condition": "has(hop.route) && hop.route == 'page'"}
  ]}}
```

Read it in five classes. The first four edges are the **observer** fan-out into
the container, and they carry **no channel guard**: an app of a person hears
that person's turns and answers whatever carried them, an operator's errand
included — the form a live colony was corrected into, after the guarded form
this README used to publish had left the app deaf to every answer that named no
channel. The third of them is the price of that: a regular edge suppresses the
guarded DEFAULT `./assistants -> .` on every case it fires on, so the unguarded
answer observer brings the channel-less exit with it as a regular edge, and an
answer that names no channel still leaves the level exactly once. The next five
**bind** the app to its container — the listened lanes, the section edge of
[#607](https://github.com/mmeyerlein/meclaw/issues/607) (the level carries every
non-`memory` section into the container, and THIS edge reads it by name,
because the installer is the one act that knows both the app's instance name
and the section it offered), the owner edge back from the screen, the view edge
out with `withdraw` beside `view`, and the exit that stamps who answered. Then
the **v-lanes** (ADR-0020): the call and the menu tick end on the connect point
the app declares for itself, drawn from BOTH surfaces of the generation —
`talky` for speech and `talky-chat` for the typed conversation, so a tool is
callable from either — and the two observed `tool_result` lanes are fan-outs
that leave the existing answers untouched. Last, the **device**: what the app
sends to `./<device>` is restamped onto the device's own door, and what comes
back is plain. The device has to stand before the edges do — an edge onto a node
that is not there routes into the dead letters.

Two more things belong to the same act. The voice channel is told
`emit_partials: true` by override — a lane nobody ordered carries nothing, and
`partial` is off by default. And the app answers a menu tick with its **whole**
offer rather than with the names it was asked about: the collector merges the
rows of every answerer, so a tool of an app reaches the brain's menu without the
surface, its collector or the grow recipe being touched at all.

A **second** installation into the same member draws the observer edges again
and they do not double: an edge is identified by its endpoints, its condition,
its modifier and its default phase, and an identical one is held once — so the
commit is idempotent and the container gets one copy of each turn. Every app
installed by the recipe draws them identically; only its own bindings are new.

### A channel may offer a tool too

The same mechanism, at a second rim. `tool`, `schemas`, `tool_result` and
`tool_schemas` dock at `./channels` as well as at `./apps`, and this level ships
the two restamp edges `./channels -> ./assistants` beside the two it already had
off `./apps`: a channel's `tool_result` becomes `in_tool`, its `tool_schemas`
becomes `in_menu`.

It exists because a channel can have something to offer that only it can do. A
telephone (`freeswitch`) is the first: it can *ring somebody up*, and the tool that
says so belongs where the line is, not in a hive beside it. Making the apps rim
the only place a tool may come from would have meant either a second app that
reaches back into the channel, or a special case — and the declaration costs
nothing on a member whose channels offer nothing, exactly like the apps pair:
without a channel that answers, nothing ever arrives on either edge.

The v-lanes in are the installing manifest's, as always
(`templates/freeswitch/README.md` § *Wiring it into a member*). What lives here is the
DECLARATION and the way back.

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
| `in_pack` | `<member>/affinity`, a **sibling** of the container (GH #458). Producer and consumer are both inside this member, so the push edge is drawn from one to the other — and since GH #561 it is a **v-lane** that ends at the generation's two brain rims, `<member>/assistants/<agent>/talky` and `…/cogny`, because the assistant level declares them as the lane's connect points and stopped carrying the pack itself. A lane at this level's own door would promise something nothing outside ever sends. |

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
(`hive_path_is_wired`). This member addresses `./assistants` on thirty-three of its
edges and `./channels` on twelve, so both containers are wired the moment the
member is instantiated — and from then on every lane they declared would owe a
`door_exists`: a message arriving at the container path must reach a cell
*inside* it. An empty container has no inside. The violation would be collected
on **every** mutation of the colony, not only on one that touches this member, so
a contract here would lock the colony for exactly as long as this member has no
assistant — or no channel — yet, which is a perfectly ordinary intermediate
state, and the normal one for `channels` on a fresh member.

The rule, which holds for all four levels: **a container hive that its own level
wires declares no `params.contract`. The transit lanes are declared by the level
whose own edges satisfy the door and exit check from birth.** Since 1.6.2 that is
literally what the apps rim does: the observer lanes are declared at the LEVEL,
with `at: ["./apps"]` naming the container as the address they dock on — the
same form `recall` and `in_bundle` use for `./assistants` — so the container
itself still declares nothing and still owes no door. A container nobody
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
- **No `terminal`, and no sink for a refusal.** See the refusal lanes above. And
  since 1.6.0 no sink of any kind: the one cell this level owned wrote files for
  its holders, and #555 gave every store its own.
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

`1.9.3` takes the **third** digit: no lane or declaration of this level moved, and
the one edge that changed repairs a promise. It pins [`memory-hive`](../memory-hive/)
at 3.4.1, whose exits now delete the five recall keys this level's door sets on the
way in ([#823](https://github.com/mmeyerlein/meclaw/issues/823)), so the bundle and the
refusal arrive back here without the question that produced them. And the door into
`./assistants` routes a session keeper's import part on its PATH
(`hop.import_hive.endsWith('/session-keeper')`), because a keeper now files under
`<talky>/session-keeper` ([#712](https://github.com/mmeyerlein/meclaw/issues/712)) —
the repair of what 1.5.0 promised.

`1.9.0` takes the **second** digit, and by the plain rule: a caller can wire
something it never could. Two edges arrive and none leaves, so sixty-four become
**sixty-six**. `./channels -> ./assistants` carries a `delegation` straight to
the generation as `in_delegation`, round the firewall, whose exit would have
made a turn of it; `./assistants -> ./channels` carries the three advice
sections back down as `in_advise`. One existing edge is widened rather than
moved: the one into the firewall promotes `context.engine` beside
`context.channel`.

Nothing is taken away, so a parent wired at `1.8.0` is still wired correctly —
the rim lists do not move at all, because both new edges have both ends inside
this level. What a colony on `1.8.0` does not have is a channel whose model
answers on its own timeline, and the two lanes that such a channel needs.

`1.7.0` takes the **second** digit, and by the plain rule: this level does
something it never promised before. `sidecar` is a new lane
([#607](https://github.com/mmeyerlein/meclaw/issues/607)) — one section of the
block a front model appends to its answer — and this level is what SORTS it:
`memory` upward into the memory hive on the door `extraction` used to use,
everything else into `./apps`, where an installed app's own edge picks the
section it answers for. The lane is declared with `at: ["./apps"]`, so the rim
lists do not move: a parent wired at `1.6.3` is still wired correctly and sees
the same eight inbound and thirteen outbound lanes. What it does not have is a
person whose apps can be written to without a tool round.

Two edges arrive and one leaves: sixty-three become **sixty-four**. The one that
leaves is `extraction` — `talky` (5.1.0) renamed the port, so `assistant` (2.6.0)
cannot raise it any more and an edge for it would be an edge nothing travels.
What survives the rename is the SHAPE: `memory-hive`'s ingress still reads the
block out of a turn as well as out of a section's `payload`, so a colony is
rewired in two steps rather than one.

`1.6.2` takes the **third** digit, and the reason is the plain one: nothing a
parent wired against moved. The rim keeps its eight inbound and thirteen
outbound lanes; what came is four DECLARATIONS that are explicitly not rim lanes
— `turn` and `partial` as emits, `tool_result` and `tool_schemas` as accepts,
all four `at: ["./apps"]` — and two edges out of that container, which restamp
an app's answer and its offer into `in_tool` and `in_menu` the way the memory's
pair has since 1.6.0. The two edges may live in the library because without an
installed app nothing ever arrives on them.

What is deliberately NOT here is the other half: the observer edges
`./firewall -> ./apps`, `./assistants -> ./apps` and `./channels -> ./apps`, and
the pair that binds an app to the container. Those are drawn by the mutation
that installs an app (see *Installing an app* above), because whoever listens
orders the lane — the rule a voice channel already follows with `emit_partials`.
Shipping them here would give every member of every colony three edges into an
empty container, and a member with no app is the ordinary state.

`1.5.0` carries a second addition that is no traffic at all: two SENTENCES, and
they ride in the same unreleased number as the `access` occupant above them for
the reason `docs/development-rules.md` § 4 gives — a version is a shipped fact,
and a `1.6.0` cut for the second half of one wave would invent a version nobody
could ever have wired against. `recall` and `in_bundle` are declared here now
([#562](https://github.com/mmeyerlein/meclaw/issues/562), ADR-0020), with
`at: ["./assistants"]` on both. Neither is a lane of this rim — both ends of that
road are inside this level, `./assistants` on one side and `./memory-hive` on the
other — and `at` says so, which is why a namespace above this one does not
declare them and why the door check does not ask this level for a rim door it
must not have. What the declaration buys is the mandatory hop: under ADR-0020 a
level that declares a lane takes part in it and may not be skipped, and taking
part is exactly what the edge `./assistants -> ./memory-hive` does — it turns
`recall` into `in_query` and stamps `audience_now`, `channel` and `recall_as_of`,
the three keys the hive refuses a question without. A v-lane drawn from a
generation's asker to any memory outside this member is now refused with
`v_lane_mandatory_hop` at mutation time instead of arriving as `missing_audience`
at runtime. **The stamping edges themselves did not move**, and that is the point
of the version: a parent sees the same seven inbound and twelve outbound lanes
across the boundary, and the level it wires them to has stopped being skippable.

`1.5.0` takes the **second** digit, and for the plain reason: a caller can now do
something that was never promised before. The level holds an `access` of its own
(GH #560), so a person's own provider keys can live with the person and a brain
four levels down can be wired to them in one edge per direction. The lane lists
at the boundary do **not** move — a credential v-lane is a deep edge into this
level's subtree, not a rim lane — so a parent wired at `1.4.0` is still wired
correctly; what it does not have is a broker of its own. See *The credential
v-lanes* above.

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
real all the same — the lane fans out to three holders, each files by its own
hive name, `export_done` travels three times with `hop.export_hive`, `hop.import_hive`
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
