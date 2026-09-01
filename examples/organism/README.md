# examples/organism

An empty folder, the template library, and **five declarations**. Out of that: a colony
shell, an organisation, a person, one generation of that person's agent, and a Telegram
channel that person is reached on — **88 cells and 497 edges**, of which **53 edges were
written by hand**.

`meclaw-os` is the example that grows *one agent* from templates. This one grows the whole
**stack**: four levels of composition, each instantiated into the level above it, each
bringing its own interior with it.

> **Measuring stick (GH #302, set 2026-08-19):** *a rebuild that takes ~30 minutes collapses
> to seconds.* The thirty minutes were not typing — they were instantiating a tree by trial
> and rejection, and re-copying every leaf bump up a chain of byte copies. Five declarations
> and one `curl` each is what is left of it.

## The rule

**A level owns what its siblings must share.**

All four levels are authored under that one sentence, and all four READMEs repeat it in the
same words. GH #302 gives three instances of it, taken from the layout that was already
running before the rule was written down:

- **memory belongs to the member**, because two assistants of one person must know the same
  person;
- **the firewall sits outside the generation**, because two channels need one view of an
  attacker and the rate window must not restart with a generation;
- **the reasoning core and the tools belong to the assistant**, the conversation surface with
  them;
- **the channels belong to the member** (GH #454, ruling R2), because one bot has to reach two
  agents of one person and a screen both of them draw on has no owner one level down.

Read the four levels downward and the rule reads off them: the shell owns the capability
broker and the control loop (every organisation asks the same broker); the organisation owns
a name and a boundary and nothing else (a group is an audience, not a holder); the member
owns the memory, the curated record, the screen and the channels; the assistant owns its
conversation surface, its reasoning core and its tool surface.

## What is checked in

```
organism/
├── seed/                      the --root of the colony. This is the whole tree.
│   ├── colony.json            substrate defaults. two lines.
│   └── main/config.json       type: "hive", and its graph is EMPTY
├── seed-ref/                  the same, plus ONE declaration that grows the shell
│   ├── colony.json            byte-identical to seed/colony.json
│   └── main/
│       ├── config.json        byte-identical to seed/main/config.json
│       └── os/config.json     type: "ref", template: "meclaw-os@1.7.0"
├── grow-os.json               1. the shell.        1 node,  0 edges
├── grow-org.json              2. an organisation.  1 node, 18 edges
├── grow-member.json           3. a person.         1 node, 18 edges
├── grow-assistant.json        4. one generation.   1 node, 14 edges
├── grow-channel.json          5. a Telegram channel. 1 node, 3 edges, born asleep
├── grow-screen.json           beyond the five: a screen and an app. 3 declarations,
│                              2 nodes, 5 edges — one storey each, so one manifest
└── grow.manifest.json         all five, in one body, in that order
```

**Zero cells.** Not a door, not a brain, not a store, not a screen — every one of them
arrives from `templates/`, at runtime, into a colony that is already up. That is the seed
principle of GH #26: a tree is grown, not checked in.

## What grows

```
/os                                 meclaw-os@1.7.0   the shell
├── access                            → access@2.4.3        the capability broker
├── argus                             → argus@1.0.0         the control loop
└── orgs                              (empty container)
    └── acme                       org@1.3.0         a namespace and a boundary
        └── members                  (empty container)
            └── alex               member@1.4.0      one person
                ├── affinity          → affinity@3.2.0      identity and meaning
                ├── firewall          → firewall@2.2.0      the screen
                ├── memory-hive       → memory-hive@3.1.0   what was said to them
                ├── channels          (empty container)
                │   └── telegram      telegram-connector@2.0.1   how alex is reached
                └── assistants        (empty container)
                    └── scribe    assistant@2.3.0   one generation of an agent
                        ├── talky     → talky@4.5.1         the conversation surface
                        ├── cogny     → cogny@4.5.0         the reasoning core
                        └── tools     → tools@1.4.0         the tool surface
```

Five `add_nodes` entries name five templates, and **seventeen** distinct templates end up stamped
on the registry's leaf rows — because a level is a composite and a composite resolves through
`ref`s. The four levels themselves carry no cell at all, so they never appear as a leaf stamp;
they appear in the **chain**. `<scribe>/cogny/collector/assemble` records
`[["assistant","2.1.0"],["cogny","4.3.0"],["collector","3.3.0"]]` — outermost first, its own
template last. An update addressing `assistant` finds that cell through the first hop, one
addressing `collector` through the last. That is the second acceptance bullet of GH #302, and
it is the question GH #277 could not answer at all.

## The five declarations, and why they are five

Each level is instantiated into the **open container** the level above ships for it — `orgs`,
`members`, `assistants`, `channels`. A container is a real hive with no children, no ports and
no contract, and its whole job is to be an address that already exists so the next mutation
has somewhere to put something.

That is also why these are five separate mutations rather than one. A container carries **no
`params.contract`** (driver ruling W7-R2), and it carries none precisely so that each level can
stand alone: a lane declared on a container would owe a door to a cell *inside* it, an empty
container has no inside, and from the first instantiation onwards that declaration would refuse
**every** later mutation of the colony until something stood there. A member with no channel
yet is a legitimate intermediate state, and this example is the proof — step 4 commits, step 5
is a separate act.

### 1. `grow-os.json` — the shell

```json
{"scope": "/",
 "diff": {"add_nodes": [{"name": "os", "template": "meclaw-os@1.7.0"}],
          "add_edges": []}}
```

One node, **no edges at all**. The shell is the outermost boundary: what reaches it comes from
outside the colony, and what leaves it leaves the colony. Everything under it — the broker, the
control loop, the forty-nine edges between them and the `orgs` container — came with the template.

### 2. `grow-org.json` — an organisation

Instantiated into `/os/orgs`, with the transit lanes in the **same** mutation, because a hive is
an island until an edge crosses into it. Six doors down (`in_turn`, `in_recall`, `in_brief`,
`in_propose`, `in_build_result`, `in_export`) and twelve exits back up (`answer`, `bundle`,
`ack`, `reject`, `error`, `write`, `turn_write`, `prune`, `build`, `close_report`,
`export_done`, `pack_ack`). Eighteen edges, and every one of them lands on the organisation's **own path**
— never on anything inside it.

The six doors also carry the organisation's **name**, and the eleven exits do not
([#478](https://github.com/mmeyerlein/meclaw/issues/478)). An exit has one destination — the container it came out of — while a
door has as many as there are children, and edges fan out: a lane guarded on
`hop.route` alone delivers to every organisation in the colony at once. The guard
is permissive (`!has(context.org) || context.org == 'acme'`), so a message that
names nobody still travels exactly as it did before there were two.

The six doors also carry the organisation's **name**, and the eleven exits do not
([#478](https://github.com/mmeyerlein/meclaw/issues/478)). An exit has one destination — the container it came out of — while a
door has as many as there are children, and edges fan out: a lane guarded on
`hop.route` alone delivers to every organisation in the colony at once. The guard
is permissive (`!has(context.org) || context.org == 'acme'`), so a message that
names nobody still travels exactly as it did before there were two.

The seventh lane `org@1.3.0` accepts, `in_import`, gets no edge, and that is the one
subtraction in this set: a memory part on its way back into a running hive addresses the
member it belongs to **at its own path**, so an edge from the container could never deliver
one. Lane count is not edge count, and this is the direction where it costs a lane rather
than an edge ([#470](https://github.com/mmeyerlein/meclaw/issues/470)).

```json
{"scope": "/os/orgs",
 "diff": {"add_nodes": [{"name": "acme", "template": "org@1.3.0"}],
          "add_edges": [{"from": ".", "to": "./acme",
                         "condition": "has(hop.route) && hop.route == 'in_turn' && (!has(context.org) || context.org == 'acme')"},
                        {"from": "./acme", "to": ".",
                         "condition": "has(hop.route) && hop.route == 'answer'"}]}}
```

**The spelling matters, and it is the one thing worth copying exactly.** A level
**declares itself AT the container it grows into**: the scope is `/os/orgs`, the node is named
bare, and the two endpoints are `.` — the declaration's own scope — and `./acme`.

*This is a correction, and the sentence it replaces was true when it was written.* Through
`builder@1.4.2` this file said the opposite: scope the level *above* the container, the node's
`name` carrying a `/`, because `"scope": "/os/orgs"` with `"from": "."` was rejected with
`edge_schema` (`from='.' unknown`) — a mutation endpoint was resolved as a node name and no node
is called `.`. [#487](https://github.com/mmeyerlein/meclaw/issues/487) fixed that: `.` and `./`
now resolve to the declaration's own scope in `add_edges`, exactly as they do in a template's
`params.graph`. [#503](https://github.com/mmeyerlein/meclaw/issues/503) is why it matters here.
The scope root is what the capability broker judges, the shipped `colony.mutate.default` rule
permits `/os/orgs` and below, and the FIRST organisation of a colony is grown at `/os` — so this
one declaration, in its old form, was the only level a colony could not build through its own
front door. Both forms grow the same tree; the absolute edges are identical to the byte.

### 3. `grow-member.json` — a person

The same eighteen lanes, one level down, into `/os/orgs/acme/members`, and the
same address on the six doors — `context.member == 'alex'` where the organisation
above reads `context.org`. The member brings its three
holders (`affinity`, `firewall`, `memory-hive`), its `export-sink`, its three open containers
(`assistants`, `channels` and `apps`) and its own fifty-one edges with it.

### 4. `grow-assistant.json` — one generation

Fourteen edges: five down and nine up.

Down are `in_turn` (the screened turn coming back off the member's firewall), `in_bundle` (the
memory hive's answer), `in_build_result` (the builder's answer) and — since GH #475 — the two
transfer lanes `in_export` and `in_import`. Up are `answer`, `recall`, `extraction`, `write`,
`turn_write`, `prune`, `error`, `build` and `dump` — the member consumes `answer`, `recall` and
`extraction` itself, files `dump` in its `export-sink`, and passes the rest on.
`assistant@2.3.0` emits a tenth lane, `pack_ack` (GH #458), and this walkthrough draws no edge
for it: nothing here pushes an identity into the generation, so nothing here produces the
receipt. A colony that wires the push wires the receipt with it, and the member already declares
the exit.

**The three transfer edges are what a generation costs beyond a conversation** (GH #475/#476).
A generation holds one store the member cannot recompute — the session ledger of its own
`session-keeper`, four levels down — so an export that names the generation on
`context.assistant` reaches it and its parts come back out on `dump`. The two doors carry the
same guard the turn doors do, because a member with two generations holds two ledgers and they
are not one document; the `dump` edge is deliberately **plain**, since every level between here
and the keeper pairs `in_export` with `dump` in `params.required_drains` and an edge that also
tested `hop.dump_kind` would read as no drain at all.

`answer` is where `assistant@2.3.0` differs from its predecessor (GH #454). The old level emitted
`turn`: its connector stood *inside* it, so the raw wire and the reply never crossed the level
boundary. The connector now stands one level up, in the member's `channels`, so the raw wire
never touches this level at all and the finished answer has to leave it. Removing an address and
a lane is a first-digit change — neither rule of `docs/development-rules.md` § 4 covers a removal.

**The four lanes that deliberately do not cross the member** (driver ruling W7-R5): `in_advice`,
`in_sweep`, `in_prune` and `in_round_sweep` are operator and timer traffic. Their producer is the
reasoning core inside the assistant, a second agent beside it, or an operator — and each of those
addresses `<member>/assistants/<agent>` **at its own path**. That is legal because neither the
member nor the assistant declares `params.ports`, so both are open, and the port boundary refuses
an outside endpoint only for a *sealed* hive. Housekeeping arrives at the agent, not through the
person.

The two doors are guarded on `context.assistant`, because a member may hold more than one agent
and the container fans in to all of them. The channel stamps the name on the way up and always
stamps *something* — see the address rule below — so a turn reaches none of them only when it
names an agent that is not there, and then it becomes `no_route` in the dead-letter queue:
recorded and self-localising, which is the honest state (2) of GH #284.

The one token that is **not** an address key is `"agent:scribe"` inside `audience_set`. That is
an audience namespace — who was in the round — and it keeps its spelling.

`ctx.model` feeds two brains since 2.0.0, because the conversation surface travels inside the
level now: `<scribe>/talky/brain` and `<scribe>/cogny/brain` both read it. Give the surface a
model of its own with `override_params` on `<assistant>/talky/brain` if the two should differ.

### 5. `grow-channel.json` — a Telegram channel of the person

**One instantiation, one mutation — and one level higher than it used to be.** Since GH #454 a
channel belongs to the person, not to a generation, so this step is declared at the *member's*
`channels` container and the node is `telegram`. The name is no label: it is the value
`context.channel_node` carries, and it is what the answer is routed back by. Nothing stands beside it — the
conversation surface travels inside `assistant@2.3.0` as `talky`.

**It is born asleep.** The entry carries `"birth": "inactive"` (GH #437), so the node is
registered, addressable and taskless: it exists in the topology before its upstream is real, and
nothing polls Telegram until a second, deliberate act arms it. That is the default the recipe
renders for this level and for no other (GH #472) — a connector opens its upstream the moment it
has a task, and every other level is a composition of cells that wait to be addressed. A wish
that wants the channel awake says so and gets `"birth": "active"` written out.

**The round is told, never derived.** `ctx.member_person` is the one `ctx` key this
level requires, and `alex` in this example is a person who happens to share a
spelling with the folder she stands in. That coincidence is what hid
[#517](https://github.com/mmeyerlein/meclaw/issues/517) for four releases: the
recipe used to read the member's identity off the **directory name**, which is
right until a member folder named after the agent holds a person called something
else — and then the ingress edge declares a round no row of the store carries,
the audience gate refuses every one of them, and the agent answers *"there is
nothing in my memory about that"*. A wish that does not name the person renders
nothing at all and comes back as `wish_incomplete` with the question.

Three edges, and there is no fourth:

- **up, the turn** — `./telegram → .` on `!has(hop.error_code)`, re-stamped to
  `turn` and carrying the round with it: `context.channel_node` (the node the answer comes
  back to), `context.channel` (the CHAT it is in — `has(hop.chat_id) ? hop.chat_id : ''`,
  GH #522), `chat_id`, `user_id`, `audience_set`
  (built from `ctx.member_person` and the `assistant` the wish named, so the claim
  stands in the manifest a reviewer reads), and `context.assistant`, the name of
  the agent the message was meant for. Every hop key it promotes is read
  `has(...) ? ... : ''`: the connector emits its own failures on this same wire
  and they carry no chat, and a modifier that fails to evaluate skips the whole
  edge;
- **up, the failure** — the same wire on `has(hop.error_code)`, re-stamped to `error`. An
  emission carrying `hop.error_code` is the connector's own failure, one without it is an inbound
  turn (the `telegram-connector` cell, GH #303), and the member re-emits the first on its own
  `error` lane;
- **down, the answer** — `. → ./telegram` on
  `hop.route == 'answer' && context.channel_node == 'telegram'`. It is the NODE name and not
  the chat: `Edge.to` is a static path and a container may hold several channels, so the way
  back has to say which child it is for ([#522](https://github.com/mmeyerlein/meclaw/issues/522)).
  The `chat_id` the first edge promoted is what the reply has to go to.

**The nine edges between `channels` and its siblings are not among them** — they belong to
`member@1.4.0` and were drawn once, when step 3 ran: `./channels → ./firewall` turns the raw
`turn` into `in_turn`, `./assistants → ./channels` carries a finished answer back to the channel
that asked, `./apps → ./channels` carries an app's `view` the same way, `./channels → .` lets a
connector's own failure leave the level, and five more place a screen's `event` and `receipt` —
two into `./assistants`, two into `./apps`, and one out on `error` for an owner this level cannot
place. A second channel does not move that number, which is #303's ruling read one level up.

#### The address rule, v1

One bot, two agents, told apart **by name** — and never by a model. The outbound edge of the
channel stamps `context.assistant` with one CEL expression and three cases:

```text
has(context.assistant) && context.assistant != ''
  ? context.assistant
  : (has(hop.addressed_to) && hop.addressed_to != '' ? hop.addressed_to : 'scribe')
```

1. an `assistant` already on the context wins — that is an operator or a test addressing an agent
   deliberately;
2. otherwise the name the connector parsed out of a prefix or a mention onto `hop.addressed_to`;
3. otherwise the channel's **default agent**, written here as the literal `'scribe'`.

The default is a literal on purpose. A CEL guard is evaluated by the substrate against `hop` and
`context` and cannot read a node's `params`, so "the default of this channel" has nowhere else to
live than the edge that applies it. Change the default and you edit this edge — which is also the
place a reader looks for it.

## A second agent, a second channel

Both are one instantiation with their own parameters, and neither re-runs anything:

```json
{"scope": "/os/orgs/acme/members/alex/assistants",
 "ctx": {"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
         "model_surface": "${MODEL_SURFACE}"},
 "diff": {"add_nodes": [{"name": "aide", "template": "assistant@2.3.0",
                         "override_params": {"cogny/brain": {"temperature": 0.9}}}],
          "add_edges": []}}
```

— plus the same fourteen transit edges `grow-assistant.json` draws, with `scribe` read as `aide`
in both the endpoints and the `context.assistant` guards.

**What a second assistant costs: one edge per direction**, inside the `assistants` container and
guarded on `context.assistant == 'aide'`. Edge targets are static in this substrate — `Edge.to` is
a path — so there is no single edge that means "send it wherever the context says", and that is a
rule rather than an accident. The channel does not learn about it: it stamps a name, and the
container decides who that name belongs to.

**What a second channel costs: one node in `channels` plus its three edges**, and the assistant
learns nothing at all. A turn from any channel arrives on the same `in_turn` door and the answer
leaves on the same `answer` lane, with `context.channel_node` telling the member where to send
it back.

The member's own twenty-one edges to and from its `assistants` container stay at twenty-one, and its
nine to and from `channels` stay at nine. That is what makes each of them one instantiation.

## A screen, and an app that draws on it

`grow-screen.json` is beyond the five, and it is the same act twice: a **display** into
`channels`, and an **application** into `apps`. Two containers are two storeys, and since
[#503](https://github.com/mmeyerlein/meclaw/issues/503) a level declares itself at the container
it grows into — so what used to be one mutation with two nodes is **one manifest with three
declarations**, and the third is the extra route into the generation the example draws by hand.

```json
{"manifest": [
  {"scope": "/os/orgs/acme/members/alex/channels",
   "diff": {"add_nodes": [{"name": "display-desk", "template": "display@1.0.0",
                           "override_params": {"web": {"port": 7902}}}], "…": "…"}},
  {"scope": "/os/orgs/acme/members/alex/apps",
   "diff": {"add_nodes": [{"name": "colony-view", "template": "colony-view@1.0.0"}], "…": "…"}},
  {"scope": "/os/orgs/acme/members/alex/assistants",
   "diff": {"add_edges": ["…"]}}]}
```

**A screen is a channel**, so it costs what a channel costs: `event` and `receipt` up into the
container, the screen's own failure up as `error`, and one edge down. Only that last one says
anything the Telegram edges do not — it takes `answer` **or** `view` and re-stamps both to the
display's own `in_view`:

```text
. → ./display-desk        (declared at <member>/channels)
  on (hop.route == 'answer' || hop.route == 'view') && context.channel_node == 'display-desk'
  set_hop.route = 'in_view'
```

That is why **the smallest view needs no app**. An agent's ordinary `answer`, carried back by
the `./assistants → ./channels` edge `member@1.4.0` already ships, *is* a view once the channel
on the other end is a screen. An agent that only wants to show a paragraph does not have to
become an application first.

**An app has no port and no surface of its own — it writes views.** Whatever authentication
stands in front of the screen stands in front of it. `colony-view` is also display-*blind*: it
emits `view` and names no display, so the screen it draws on is one literal in the edge that
leaves it (`set_context.channel_node = 'display-desk'`, with `channel` carrying the same word —
a screen is one room, so its address and its conversation partner are the same). Wire it to two screens and it appears on
both, and neither is named in the app.

**The way back is the owner, and only the owner.** The display stamps `hop.owner` — the path of
the cell whose `reply_to` put the view up — on every `event` and every `receipt`. The member
splits on the container (`/assistants/` → the agent, as an ordinary `in_turn` carrying
`hop.kind`; `/apps/` → the app, lane name kept; neither → out on `error`), and this file adds
the two edges that say *which* one, guarded on `hop.owner.contains('/apps/colony-view/')` and
`hop.owner.contains('/assistants/scribe/')`. One edge per recipient, the same static-`Edge.to`
cost a second assistant has ([#459](https://github.com/mmeyerlein/meclaw/issues/459)).

## The numbers, measured

| what | how many |
|---|---:|
| cells checked in | **0** |
| cells after the five declarations | **88** |
| edges after the five declarations | **492** |
| edges written by hand in the five files | **51** |
| edges that came with a template | **441** |
| `add_nodes` entries | **5** |
| distinct templates stamped in the registry | **17** |
| edges between the member's `channels` and its siblings | **9** |
| edges between the member's `assistants` and its siblings | **20** |

Re-measure them with

```text
cargo test -p meclaw-cells --test gh302_the_stack_grows_from_templates \
    print_the_measurement -- --ignored --nocapture
```

Every hand-written edge in these five files lands either on a node the same declaration
instantiates or on the open container it is instantiated into. **Not one reaches into an
interior**, and that is the first acceptance bullet of GH #302, asserted in
`crates/meclaw-cells/tests/gh302_the_stack_grows_from_templates.rs`.

## Run it

```bash
# from the repo root, on a fresh release build
cargo build --workspace --release

cat > examples/organism/seed/.env <<'ENV'
OPENROUTER_API_KEY=sk-...
MODEL_CORE=openai/gpt-4o
MODEL_CORE_FAST=openai/gpt-4o-mini
MODEL_SURFACE=openai/gpt-4o-mini
MODEL_CLOSER=openai/gpt-4o-mini
MODEL_DIALECTIC=openai/gpt-4o-mini
MODEL_DREAMER=openai/gpt-4o-mini
TELEGRAM_BOT_TOKEN=...
ENV

./target/release/meclaw --root ./examples/organism/seed \
                        --templates ./templates \
                        --daemon --api 127.0.0.1:7777
```

Open `http://127.0.0.1:7777/ui/registry`. **Nothing.** Now grow it, in order:

```bash
for step in os org member assistant channel; do
  curl -s -X POST http://127.0.0.1:7777/colony/mutations \
       -H 'Content-Type: application/json' \
       -d @examples/organism/grow-$step.json
done
```

Reload the registry. Eighty-two cells.

Every credential in this folder is a `${VAR}` token and never a value; the substitution reads
`{root}/.env` (or `--env`), not the process environment. A variable with no default has to
exist before the declaration that reads it will commit — `TELEGRAM_BOT_TOKEN` carries none, so
step 5 is rejected with `env_var_missing` until the line is there. Any value gets the mutation
through; a real one is what makes the bot poll. One `getUpdates` consumer per bot token: a
second poller on the same token gets 409 and the two steal each other's updates, so a second
channel wants a second bot.

The broker and the control loop start **inert by design** — every seeded policy row ships
disabled and every charter goal ships disabled — so a fresh shell grants nothing and changes
nothing until an operator turns on exactly what they mean.

## The seed that grows itself

`seed-ref/` is the same seed with one thing added: a `cell.type: "ref"` marker where the shell
shall stand.

```json
{"cell": {"type": "ref", "template": "meclaw-os@1.7.0"}}
```

That is a **declaration, not a cell**. The FIRST `meclaw --root ./examples/organism/seed-ref`
resolves it against the template library and grows it — through the very resolution and staging
a mutation takes, which is why the result is byte-identical to what `grow-os.json` builds
(`crates/meclaw-cells/tests/gh424_the_seed_grows_itself.rs`). Then the marker is **gone**: what
stands in its place is the shell it named. Two consequences follow from that rather than being
bookkept — a second boot finds nothing to grow, and a node you later remove with `remove_nodes`
cannot be re-declared into existence by a restart.

**What it is not.** `seed-ref/` does not replace the five declarations, and it cannot. A `ref`
marker declares a **node** and never an **edge** — and 53 of this example's 497 edges are
hand-written transit lanes hanging off `orgs`, `members`, `assistants` and `channels`, hives the
templates themselves materialise. Until the growth has happened those addresses do not exist, so
there is nowhere to write them down. `seed-ref/` therefore grows exactly the first level, and it
is the honest form of "a tree that boots itself".

The whole stack from one file is the other half, and it is a manifest:

## One file, one command

`grow.manifest.json` is the five declarations verbatim, in the same order, in one body:

```json
{"manifest": [ …grow-os.json…, …grow-org.json…, …grow-member.json…,
               …grow-assistant.json…, …grow-channel.json… ]}
```

The colony rolls it off itself — entry by entry, each through the very validation a single
mutation gets, stopping at the **first** refusal and answering with one receipt. There is
deliberately **no rollback**: what committed stays committed, and the receipt says where to pick
it up again.

```bash
./target/release/meclaw --root ./examples/organism/seed \
                        --templates ./templates \
                        --apply examples/organism/grow.manifest.json
# applied 5 of 5 mutations.
```

Boot, apply, print, shut down — the exit code is the verdict. A refusal reads:

```text
applied 3 of 5 mutations; entry 4 was refused.
  error_code: env_var_missing
  details:    …
  the first 3 entries are committed and stay committed (no rollback).
  to resume: drop the first 3 entries from the manifest and apply it again.
```

Against a colony that is **already running**, `--apply` is refused by the root lease, and that is
the right answer: there you mutate through its HTTP door — which takes the same manifest body, so
it is one `curl` instead of five.

```bash
curl -s -X POST http://127.0.0.1:7777/colony/mutations \
     -H 'Content-Type: application/json' \
     -d @examples/organism/grow.manifest.json
```

## Not in scope

- **No sink.** Nothing here accepts a refusal and emits nothing (ruling Q2, GH #284): a lane
  with no consumer becomes `no_route` in the dead-letter queue, which is recorded and
  self-localising. What it must never have is a cell that accepts it and drops it.
- **No slot.** The substrate's slot governs an address that does **not** exist, and every
  container in this tree does exist — so the declaration would be silent, and the
  `params.ports` it needs would have *sealed* the level that declared it.
- **No second vault.** `access@2.4.3` carries its own interior one (ruling Q20).
- **No live migration.** This folder is a walkthrough for a colony that is grown from nothing.
  Running any of it against a deployed tree is a separate, operator-owned act.
