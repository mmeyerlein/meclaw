# templates/

Reference topologies you can instantiate. Every one of them is pure DSL -- directories,
`config.json` files and edges. No new Rust, no plugin API, no runtime dependency on this
folder at all.

## What a template is

A template is a physical subtree on disk: a `template.json` at its root (name, version and a
four-slot description that the colony serves over `/colony/templates`), one `config.json` per
cell, and sub-cells as nested directories. That is the whole format. What you read here is
exactly what the substrate reads.

**Instantiation COPIES.** When a mutation names a template, the colony materializes the subtree
into your colony root -- substituting `${uuid7:*}` on disk, keeping `${VAR}` as a late-binding
token -- and from that moment your instance is yours. It has no link back to this library. This
folder is not on the runtime path of a booted colony; delete it and every colony that was built
from it keeps running.

A template is also self-contained: it has no edges leaving its own subtree. Wiring it into a
colony is the job of the mutation that instantiates it (see below).

## The hive boundary

Every template that is a hive is bound by one rule, and it is a requirement rather than a
convention:

> An edge is laid at the HIVE. Access from outside happens abstractly and functionally, never
> structurally and directly. An edge asks for something by content, without knowing the
> structure. The inner edge that receives that request is what knows what to do about it.

Ruled 2026-08-18 ([#197](https://github.com/mmeyerlein/meclaw/issues/197),
[#200](https://github.com/mmeyerlein/meclaw/issues/200)) and specified in
[`../docs/meclaw-overview.md`](../docs/meclaw-overview.md) § The hive boundary. **It binds
every hive and every template** -- a hive that declares no `params.ports` is one the substrate
does not enforce it on, not one it does not apply to. Which templates have arrived and which
have not is the table in that section.

### Authoring a hive template: four things, all checkable

1. **`params.ports: []`** in the hive marker's `config.json`. The empty list is not an omission,
   it is the statement "the hive path is the only address". No `ports` key at all means unsealed,
   which means unfinished.
2. **A door per accepted lane** -- an edge with `"from": "."`, a `condition` testing the lane,
   and a `to` naming the inner cell that serves it. This is the only place the structure of the
   inside may be known.
3. **`params.contract`** with `accepts` and `emits`, and **lane names that say what the caller
   wants, never where it lands inside**. `writer`, `recall`, `render`, `policy` are inner cell
   names; renaming a port into a lane of the same name satisfies the letter of rule 1 and misses
   its point. The test: does the name survive a reimplementation of the inside?
4. **No address in your prose that the boundary would refuse.** `template.json` and the README
   describe lanes, not cells -- the `description` slots are the interface a caller reads, and a
   `from:`/`to:` in them is a wiring instruction, which is why
   [#203](https://github.com/mmeyerlein/meclaw/issues/203) was a defect and not a typo. The gate
   is `crates/meclaw-cells/tests/gh203_documented_port_addresses.rs`, and it asks the real
   boundary validator.

### Wiring one: address the hive, name the lane

An edge from outside points at the hive path and carries its request on `hop.route`:

```json
{"from": "./ingress", "to": "./agent",
 "modifier": {"set_hop": {"route": "'in_turn'"}}}
```

Which lanes a hive accepts and emits is in its `params.contract`. If you find yourself needing
a segment after the hive name, the answer is not a longer address -- it is a lane whose name
says what you wanted, and a door inside the hive that knows where that belongs.

### Writing a cell a tool round will call

A tool cell answers on the collector's `in_tool` lane, and what it may hand back is its
**`messages[]`**: one `tool_result` turn per call it answers, each carrying the `id` of
that call. All of them travel -- a tool that got a bundle of calls in one message may
answer the whole bundle in one message.

Nothing else travels. A `system` slot or a top-level body slot on that lane is dropped,
and deliberately so: `system.*` reaching an `llm` cell is upserted into that cell's own
`cell.db` and stands in the prompt until something overwrites the same slot path, which
makes it durable state of the agent rather than evidence of one round. A tool with
structure to return serialises it into the text of its result; a producer that means to
install a lasting constraint addresses the `llm` cell's `system` tree on a lane of its own
instead. The long version, with the reasoning and the worked example, is
[`collector/README.md` § What a tool result may carry](collector/README.md).

## The library

**This table is the published library, and it is a subset of what a working tree may
carry.** Every template the export ships has a row here -- that is a gate, not a habit
(`crates/meclaw-cells/tests/gh235_every_public_template_has_a_row.rs` compares the rows
against the export's own allow-list, and its twin `gh235_readme_library_table.rs` compares
each row's version against that template's `template.json`). A development tree can hold
further directories under `templates/` that are deliberately not published: they carry no
compatibility promise, they are not referenced from here, and nothing in this table
depends on one.

| Template | Version | What it is |
|---|---|---|
| [`access`](access/) | 2.3.0 | The capability broker as one hive: an agent may ask in natural language, what travels on the wire is a **handle**, and a credential leaves the vault beside it only SEALED, under a key the requester minted for that one call. Six cells, no model -- every verdict is a comparison. Ships inert: every seeded policy row is disabled, so a fresh instance grants nothing. Since 2.3.0 a request may also be CHECK-ONLY -- a verdict and no grant, because a grant nobody spends is a bearer row with an expiry date -- and a rule may name `scope_prefix`, a reserved key compared as a PATH prefix rather than for equality. |
| [`affinity`](affinity/) | 3.0.0 | The curated record of the people and agents a colony knows: whole AIeOS 1.1.0 documents in a store, plus the four things that standard has no vocabulary for -- relations, trust, disclosure and an append-only audit. One writer, one reader, no model, and the audience filter exists in exactly one place. |
| [`archive-bridge`](archive-bridge/) | 1.0.0 | Turns an llm's last answer into a store-native insert -- and swallows the store's reply echo, so an append-only archive costs one cell instead of a loop. |
| [`assistant`](assistant/) | 1.1.0 | One generation of one person's agent, as a composition level: the reasoning core (`cogny`) and the tool surface (`tools`) every channel of that generation shares, plus one open, empty `channels` container the surfaces are instantiated into. Because that container is a node of the template, the eighteen edges that address it ship ONCE (GH #303) -- a second channel costs two instantiations inside `./channels` and their pairing edges, never a re-run of the fan-in. Seven lanes in, eight out, twenty-three edges -- one of the new pairs is the REACH of the tool surface (`build` out, `in_build_result` in), declared rather than hidden (#425). No memory, no identity, no screen: those belong to the member (GH #122). |
| [`builder`](builder/) | 1.0.3 | The intake that turns a structural wish into a MANIFEST -- an ordered list of mutation declarations, ready to be submitted by whoever asked for it. Two classes on one lane: a fast lane of predefined parameterised recipes that runs without calling a model at all, and a design lane that consults `builder-librarian` and asks one. It never applies anything, and that is a property of the FILES rather than a promise in a README: no cell in it has an edge onto the control plane, and `/colony/mutations` is not an endpoint a mutation may draw on any scope (ADR-0015). |
| [`builder-librarian`](builder-librarian/) | 2.0.6 | Lexical retrieval over the builder's knowledge base, as one sealed hive: a `store` cell holding the corpus in an FTS5 table, and a `retrieve` cell that turns a request into a BM25 query and the rows back into a briefing. No embeddings and no model -- a corpus that answers to names is answered by names. Retrieval is an ENHANCEMENT: a store failure comes back marked `degraded` rather than as an honest-looking zero-hit, so a corpus outage cannot hang a build. **Its seed is a build product, and the published one is not the one a development tree carries**: the corpus shipped here is generated from the public sources alone (the spec plus this table's templates), because a corpus is a copy of its sources and this repository's are not all public (#441). |
| [`canvy`](canvy/) | 2.1.9 | An interactive canvas of the colony, on a port of its own: a timer takes a topology snapshot, a `probe` asks `/colony/graph`, a `layout` cell turns the snapshot into display objects, and a `web` cell holds them and serves the page. Since 2.0.0 canvy draws nothing itself -- it is the first application of the `web` cell, and node positions are `editable` object props, so a drag is local CRUD plus a diff to every viewer instead of a topology round trip. The browser owns the drag and the camera, nothing else. Since 2.1.8 a hand-placed cell can be handed back to the layout: the pin is a marker of its own (`pinned`), not the mere presence of a coordinate, and the detail panel releases it. Ask it for a fresh snapshot on `in_refresh`; a semantic browser event leaves on `event`. |
| [`cogny`](cogny/) | 4.0.3 | The agent core as one node: a tool loop of `collector` + `dispatcher` around an llm brain slot, every internal edge pre-wired. A talky without a channel. Asked on `in_turn`; answers on `answer`, calls tools on `tool`, asks a memory on `recall` and drains a failed inference on `error`. |
| [`collector`](collector/) | 3.0.2 | Context assembly as a hive: a rolling conversation window, the memory bundle and the tool round fanned back in -- each leg capped by configuration, not by a model's judgement. **Since 3.0.0 route `turn_write` hands out one message per turn**, not one batch of the day: a caller wired to the batch shape breaks (#298). Since 3.0.1 every junction of the assembly is ONE bundle instead of a fan-out with a guarded fan-in: opening a turn costs one store message instead of three, an assembly one instead of four, a tool round one instead of four (#419). |
| [`daily-digest`](daily-digest/) | 2.0.2 | Scheduled fetch-and-forward as one hive: a timer fires daily, a `web_fetch` pulls one URL, a `code` cell formats what came back, and a Telegram `proxy` delivers it to a fixed chat. Five cells, no model. A parent may also demand a run at the hive path on lane `in_digest`, bringing its own `context.chat_id`, and take the formatted digest back on lane `digest` — so the same template is both a standing schedule and a callable step. |
| [`dispatcher`](dispatcher/) | 1.1.1 | The fan-out half of a tool loop: splits a brain's `tool_call` bundle into one routable message per call, announces the round to the fan-in, and passes a final answer through. |
| [`door`](door/) | 1.0.2 | The first cell of a colony, as one code cell: it takes what the HTTP ingress delivers -- request headers in `context`, an empty hop -- and puts the turn on a named lane, carrying the channel identity with it. |
| [`firewall`](firewall/) | 2.0.5 | Deterministic screening in front of an agent: size, sender, forbidden literal and rate, each verdict naming the rule that fired. Nothing here asks a model, and nothing here can. |
| [`meclaw-os`](meclaw-os/) | 1.2.3 | The colony shell, as the outermost composition level: the capability broker (`access`) and the control loop (`steward`) every organisation shares, plus one open, empty container the organisations are instantiated into. Seven lanes in, nine out, twenty-seven edges, and no cell of its own. Since 1.1.0 it also holds the two halves of the one authoring path a colony has: the `builder` that drafts a manifest and the `submit` that carries it to the mutation door (#425, ADR-0015). Since 1.2.0 those two are wired to the broker as well: the submitter asks whether a manifest may be applied, and only this level can draw that pair (#435). |
| [`member`](member/) | 1.1.0 | One person, as a composition level: the memory (`memory-hive`), the curated record (`affinity`) and the screen (`firewall`) every assistant of that person shares, plus one open, empty container the assistants are instantiated into. Five lanes in, eight out, twenty edges. Two assistants of one person must know the same person and must meet one attacker, which is why all three stand here and not one level down (GH #122). |
| [`memory-drain`](memory-drain/) | 2.0.5 | The adapter between a write batch and a central memory: decomposes one batch into single-turn episodes, in order, idempotent across replays. Since ADR 0012 an **import adapter for foreign history, wired by nobody** — a live conversation writes its turns one message at a time and never passes through here. |
| [`memory-hive`](memory-hive/) | 3.0.4 | A member's long-term memory as a hive — one source of truth that every agent of that member reads: an LLM-free write path, a token-budgeted tier-0 bundle, a four-leg retrieval fan fused without a model, and a nightly consolidation that supersedes instead of deleting. Thirteen cells, no Rust. Since 2.2.0 the remembered content can also leave the hive as a versioned document and enter another running one, idempotently -- the template-level answer to #243, while #253 is the substrate one. Since 2.3.0 a tier-1 recall delivers two documents in one message: a bundle written for the model that has to answer -- provenance, currency and an explicit "nothing here answers this" -- and the retrieval's own record beside it, for whoever has to explain the answer afterwards. Since 2.3.4 a tier-0 recall costs ONE store round trip for its three legs instead of nine, and parks nothing; since 3.0.2 a tier-1 recall costs SIX instead of 47, measured, with the answers byte-identical across the rebuild (#418). **Since 3.0.0 there is exactly ONE lane that writes facts mid-conversation** — the answering model annotates the turn it just answered; the batched `extractor`, its gate and the `in_flush` lane are gone (#298). Behind that ingress stand two readers and no second writer: the night, and a close pass that reads an ended session whole on a strong model (`in_close_pass`, #300). |
| [`org`](org/) | 1.1.0 | An organisation as a composition level, and as nothing more: a name, a boundary and one open, empty container its members are instantiated into. No cell at all -- five transit lanes in, eight out, thirteen edges. A level that shares nothing is still a level when what it is worth is the namespace. |
| [`receptionist`](receptionist/) | 2.0.4 | One agent per channel, built on demand: the first turn of a channel nobody has met instantiates a fresh `talky` for exactly that channel and hands the turn straight into it. |
| [`retry`](retry/) | 1.0.0 | A bounded retry loop around one tool, as a single cell. At the cap the give-up lane hands the last error on with its `error_code` intact. |
| [`session-keeper`](session-keeper/) | 2.0.4 | A session as a channel generation, modelled on a phone call: minted at the surface, stamped onto every inbound turn, ended by arithmetic (a timer plus an idle threshold) rather than by judgement. |
| [`steward`](steward/) | 2.0.12 | The colony's control loop: charter, deterministic measurement, a judge that simulates before it decides, a params update to the cell it names, an immediate health check, and keep-or-revert after the window -- every cycle a receipt. Ships with every goal disabled. |
| [`submit`](submit/) | 2.0.0 | Two occupants behind one door, and the only reach onto the mutation door in the whole tree. It checks that a manifest's bytes are the ones its digest was drawn over, takes the requester's identity off the ENVELOPE rather than out of the body -- and then ASKS: who may submit is a check-only question to the capability broker, put once per submission over the manifest's scope root, while the manifest waits parked in the store beside the gate. A permitted manifest is un-parked, stamped with its attribution and emitted once. The colony's answer comes back directly and becomes a receipt that carries the id of the call that asked for it, because the same store also remembers the round in flight. |
| [`summarizer`](summarizer/) | 2.0.2 | The handover step: when a generation closes, it folds the day's write batch into one recency-weighted summary and emits it as a `system.handover` update. |
| [`talky`](talky/) | 4.2.3 | The full composite agent: `session-keeper`, `collector`, `dispatcher` and `summarizer` around an llm brain slot, with the loopback, the close path and the handover return already wired. Since 4.1.0 a `splitter` sits on the answer path and cuts the extraction sidecar out of an annotated answer onto its own `extraction` lane — per-turn memory extraction stopped being a tool call (#379). Without the extraction contract in the brain's instructions that cell is a pure pass-through. |
| [`telegram-connector`](telegram-connector/) | 2.0.1 | A Telegram chat as one address: one `proxy` cell that owns the chat credential, taken verbatim from `bot-basic@2.0.0`. A turn in from the chat and a finished answer back, both on the cell itself; `hop.error_code` tells an inbound turn from the connector's own failure, and the level that holds it owes that failure a drain. No persona, no model, no answer of its own. |
| [`vault`](vault/) | 1.2.0 | A secret store with no operation that returns a secret -- not a policy over a store, but a cell type whose route surface has no read on it. Secrets enter over the user channel only; the broker may ask it to USE one, or to DELIVER one sealed. A vault behind a sealed hive opens itself from an environment variable it names, or not at all. |
| [`terminal`](terminal/) | 1.0.1 | The last cell of a lane, as one code cell: it accepts anything and emits nothing. Its whole job is to be an address, so that a lane without a destination yet still HAS one -- the message arrives, the trace records it, and the dead-letter queue stays empty. |
| [`tools`](tools/) | 1.1.0 | The tool surface of one assistant as ONE node with ONE contract: `tool_call` in, `tool_result` out. Sealed (`params.ports: []`), so which tools exist is a change INSIDE the hive and never a change to the caller's edges -- five tool occupants today (a sandboxed one-shot shell, a GET-only fetcher, a generic search wrapper, and since 1.1.0 the two halves of a structural build round) plus a sixth cell that turns an unknown tool name into a named refusal, and replacing the tools with one code-executing cell is a single `swap_nodes`. **`tool_result` is still the one RESULT lane, whatever the tool was**; `build` is a different class -- the REACH of the surface, declared in the contract rather than discovered in an edge table (#425). |
| [`web`](web/) | 1.1.0 | A display as one cell with a port of its own: an HTTP + WebSocket listener, an object tree and a component library in its own `cell.db`, and server-side rendering that is materialised -- a page load answers from memory and costs no cell call. Components are rows, so a model can define one at runtime. Ships the Vision design language as seed data: the token stylesheet plus nine components (`stack`, `card`, `heading`, `text`, `table`, `button`, `input`, `badge`, `ornament`) and a `/demo` page composed of nothing else. Two of its rules are refusals rather than advice -- glass is a navigation-layer material, and glass never sits on glass. Authentication is external, forever: the default bind is loopback and a reverse proxy goes in front. |

Read a template's `template.json` before wiring it -- the `description` slots (`purpose`,
`use_when`, `not_in_scope`, `examples`) say what it is for, and for a hive its `params.contract`
says which lanes it accepts and emits, which is what you need in order to connect it. `cogny`
and `talky` are composites: they build on the smaller templates as sub-units, so instantiating
one of them pulls in nothing that is not in this table.

`meclaw-os`, `org`, `member` and `assistant` are the four **composition levels**, authored
under one rule -- *a level owns what its siblings must share* -- and each of them is a
composite too: `ref`s to templates that already stand in this table, its own topology, and one
open, empty container the level beneath it is instantiated into. Instantiating the outermost
one and then filling the containers downward grows the whole stack, which is what
[`examples/organism`](../examples/organism/) does with five declarations and no hand-written
interior edge.

A directory name inside a template is the address every grown instance answers to, so an
app-hive name that collides with the substrate glossary is a review defect, not a
preference: name it `catalog`, not `registry`, and `calendar`, not `scheduler` (GH #26).
`crates/meclaw-cells/tests/gh26_app_hive_names_avoid_the_substrate_glossary.rs` refuses
those two words across the whole tree -- deliberately two words wide, because a gate with
an exception list is one authors route around, and a name is cheap to change now and
expensive after the first instance is grown.

## Instantiating one

Point a running colony at this directory and send an `add_nodes` mutation. The short form:

```bash
./target/release/meclaw --root ./mycolony --templates ./templates \
                        --daemon --api 127.0.0.1:7777

curl -s -X POST http://127.0.0.1:7777/colony/mutations \
  -H 'Content-Type: application/json' \
  -d '{"scope":"/","ctx":{},"diff":{
        "add_nodes":[{"name":"agent","template":"talky"}]
      }}'
```

Because a template has no outgoing edges, the node above lands connected to nothing -- and a
subtree that nothing crosses into derives inactive, so its long-running cells never spawn. Wire
it in the **same** mutation: one `add_edges` entry from an already active cell onto the
template's own path is enough to bring the whole subtree up on that one recompute.

And if you want it wired but NOT running -- a long-poll consumer whose upstream tolerates only
one reader, say -- declare `"birth": "inactive"` on the `add_nodes` entry: a cell born inactive
is registered, addressable and persisted inactive, and no task is built for it, not even when
the same mutation wires it. The next mutation that reaches it wakes it.

```bash
curl -s -X POST http://127.0.0.1:7777/colony/mutations \
  -H 'Content-Type: application/json' \
  -d '{"scope":"/","ctx":{"model":"openai/gpt-4o-mini"},"diff":{
        "add_nodes":[{"name":"agent","template":"talky"}],
        "add_edges":[{"from":"./ingress","to":"./agent",
                      "modifier":{"set_hop":{"route":"'in_turn'"}}}]
      }}'
```

The `ctx` block feeds the `${ctx.*}` placeholders a template declares -- `talky` wants a resolved
model literal.

**That edge is the finished shape** (§ The hive boundary). Since
[#228](https://github.com/mmeyerlein/meclaw/issues/228) every hive template that ships is sealed:
the address is the template's own path and the request is a lane on `hop.route`. Nothing here
names a cell inside a hive any more, which is what lets a template be swapped for another one
arranged differently. The lanes a hive accepts and emits are in its `params.contract`.

Working colonies built this way live in [`../examples/`](../examples/). Start with
[`hello`](../examples/hello/README.md) for the model itself, then
[`swarm`](../examples/swarm/README.md) for a tool loop. The mutation format is specified in
[`../docs/meclaw-overview.md`](../docs/meclaw-overview.md); the cell types a `config.json` may
declare are in [`../docs/cell-types.md`](../docs/cell-types.md).

## Versioning

A reference in a mutation is either `name` or `name@major.minor.patch`. With a version it is an
**exact** match (`talky@2.0.0`); without one, it resolves to **the one version registered under
that name**. **Correction (GH #277):** this line used to say "the highest version on disk wins".
That is dead under the uniqueness rule — the scan aborts as soon as two `template.json`s declare
the same `name`, *regardless of their versions*, so a scanned library holds exactly one entry per
name and a bare-name reference has exactly one answer. There is no "highest" to pick from.

Semver ranges (`^`, `~`) are not parsed today, so `talky@2` is not a resolvable reference -- when you see
`talky@2` in prose here or in an issue, it names the major line of the template, not a string you
put in a mutation.

Version numbers here move only forward, and a bump never reaches a colony that is already
running: instantiation copied the subtree, so a running instance is pinned to the bytes it was
built from. Upgrading is therefore always an explicit act -- instantiate the new version next to
the old one and move the edges -- never something that happens to you between restarts. That is
the reason the copy exists.

**Where pinning stops working today, and the intent.** A template lives in exactly one
directory, so a version bump **replaces** it: after `talky` goes to `2.0.0`, a mutation asking
for `talky@1.2.0` finds nothing and is rejected, even though that is precisely the request a
pin is supposed to survive. The pin protects a *running* instance (it already holds its copy);
it does not protect a *new* instantiation reproducing an old one. That asymmetry is a gap, not
a design. The intent: **starting with 0.9.0, superseded template versions remain available**,
so a pinned reference keeps resolving after the version it names has been superseded. Until
that lands, treat `name@exact-version` as a statement about what you built against, and vendor
the directory if you need to rebuild it later.

**Correction (GH #277) — how that promise has to be read now.** The obvious reading of "superseded
versions remain available" was: keep the old directory beside the new one
(`templates/talky-3.0.11/` next to `templates/talky-3.0.12/`, both declaring `"name": "talky"`).
That reading is no longer merely unsupported, it is a **scan error**: the uniqueness rule refuses
the second `template.json` declaring an already-seen name, regardless of version, and the whole
scan aborts — taking the boot or the `RescanTemplates` with it. The promise itself stands, but it
now needs a mechanism it did not need before: an archive or a registry that holds superseded
versions *outside* the one-directory-per-name library (a separate library root, a content store —
undecided), not a second directory inside it. Until such a mechanism exists, one directory per
name is the only shape a library may have.

## Env knobs are an experimental surface

Most templates here take their tunables as `${KNOB}` substitutions out of `.env`. That is
convenient and it is temporary: the knobs are migrating onto the `params` block of the cells
that read them, template by template, over the `0.x` line. Until a template's migration lands,
**its knob names are not a compatibility promise** -- a name may change, split, or disappear in
any `0.x` release, and the template README is the only place that says what it is called today.

The migration is tracked in
[#138](https://github.com/mmeyerlein/meclaw/issues/138); the `collector@1.2.0` migration
([#136](https://github.com/mmeyerlein/meclaw/issues/136)) is the reference pattern, and every
migration keeps its defaults bit-identical. What does **not** move: provider credentials and
endpoints stay in `.env`, because a secret in a `config.json` is a secret in the repository.

New templates are added by the same rule that governs everything else in this repository: a
subtree plus its gates, no substrate change. Contributions are welcome; see
[`../CONTRIBUTING.md`](../CONTRIBUTING.md).
